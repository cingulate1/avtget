import { create } from 'zustand';
import type { JobItem, JobStatus, ArtifactStatus, JobConfig } from '@/shared/types';
import { sanitizeUrl } from '../utils/youtube';
import { getDesktopAPI } from '../desktopApi';

interface ProcessingModes {
  video: boolean;
  audio: boolean;
  transcript: boolean;
  summarize: boolean;
}

// Mirror the five main-window toggles into config.ini immediately on every
// change. Unidirectional (GUI -> file): the file is never read back into these
// toggles except for the one-time startup seed from the default_* keys (see
// App.tsx's initial-load effect). Fire-and-forget — a write failure must never
// block the UI, and the desktop bridge may be absent outside the Tauri runtime.
function persistLiveModes(modes: ProcessingModes, verbose: boolean): void {
  try {
    void getDesktopAPI().setLiveModes({ ...modes, verbose }).catch(() => {});
  } catch {
    // Desktop bridge unavailable — nothing to persist.
  }
}

export interface ClipRange {
  start: string;
  end: string;
}

// One fully-frozen job awaiting dispatch — created per unique input at GO time.
// `config` is the complete, immutable JobConfig (single input + frozen settings
// + its own config_snapshot_path); the dispatcher hands these to the backend
// one at a time. `itemId` is the sanitized input and the key of the matching
// UI job in `jobs`.
export interface QueuedJob {
  itemId: string;
  config: JobConfig;
}

interface JobState {
  jobs: Map<string, JobItem>;
  inputs: string[];
  clipTimestamps: ClipRange[][];
  currentModes: ProcessingModes;
  isRunning: boolean;
  verboseMode: boolean;
  statusTotal: number;
  showEpisodeLimit: boolean;
  episodeLimit: number;
  // Track transcript files that need manual Ollama cleaning or should be skipped
  manualCleanInputs: string[];
  skipInputs: string[];
  // The unified job queue. Every submitted input — manual Go or Firefox intake
  // — becomes one frozen QueuedJob here and is dispatched to the backend one at
  // a time on each `backend_exited` event (see jobDispatch.ts /
  // useBackendEvents.ts); the backend runs a single process per job. Survives
  // `reset()`.
  jobQueue: QueuedJob[];

  setInputs: (inputs: string[]) => void;
  setClipTimestamps: (timestamps: ClipRange[][]) => void;
  removeInput: (index: number) => void;
  clearInputsForCompletedJobs: () => void;
  setRunning: (running: boolean) => void;
  setVerboseMode: (verbose: boolean) => void;
  setModes: (modes: ProcessingModes) => void;
  addJob: (itemId: string) => void;
  updateJobStatus: (itemId: string, status: JobStatus) => void;
  updateArtifactStatus: (itemId: string, artifact: string, status: string) => void;
  setFilestemOverride: (itemId: string, filestem: string) => void;
  setStatusTotal: (total: number) => void;
  removeJob: (itemId: string) => void;
  clearCompletedJobs: () => void;
  clearAllJobs: () => void;
  reset: () => void;
  setShowEpisodeLimit: (show: boolean) => void;
  setEpisodeLimit: (limit: number) => void;
  addManualCleanInput: (path: string) => void;
  addSkipInput: (path: string) => void;
  enqueueJobs: (units: QueuedJob[]) => void;
  requeueJobAtFront: (unit: QueuedJob) => void;
  dequeueJob: () => QueuedJob | undefined;
  clearJobQueue: () => void;
}

export const useJobStore = create<JobState>((set, get) => ({
  jobs: new Map(),
  inputs: [''],
  clipTimestamps: [],
  currentModes: { video: true, audio: true, transcript: false, summarize: false },
  isRunning: false,
  verboseMode: false,
  statusTotal: 0,
  showEpisodeLimit: false,
  episodeLimit: 10,
  manualCleanInputs: [],
  skipInputs: [],
  jobQueue: [],

  // The single sanitization point for URL entry into the app. Every path that
  // places a value in the input field (typing commit, paste, file drop, .txt
  // loader, Firefox extension intake) funnels through here. Non-URL strings
  // (file paths, raw text) pass through unchanged.
  setInputs: (inputs) => set({ inputs: inputs.map(sanitizeUrl) }),
  setClipTimestamps: (timestamps) => set({ clipTimestamps: timestamps }),

  removeInput: (index) => {
    const { inputs } = get();
    const newInputs = inputs.filter((_, i) => i !== index);
    // Always keep at least one empty input
    set({ inputs: newInputs.length > 0 ? newInputs : [''] });
  },

  clearInputsForCompletedJobs: () => {
    const { jobs, inputs, currentModes } = get();
    // Find all successfully completed jobs (including warning status which means raw transcript was saved)
    const completedJobIds = new Set<string>();

    for (const [id, job] of jobs) {
      const requested = Object.entries(currentModes)
        .filter(([_, enabled]) => enabled)
        .map(([key]) => key as keyof typeof job.artifacts);

      const allCompleted = requested.length > 0 && requested.every(
        (artifact) => job.artifacts[artifact] === 'completed' || job.artifacts[artifact] === 'warning'
      );

      if (allCompleted) {
        completedJobIds.add(id);
      }
    }

    // Filter out inputs that match completed job IDs (URLs)
    const remainingInputs = inputs.filter(input => {
      const trimmed = input.trim();
      // trimmed is already sanitized (setInputs enforces this), so it matches
      // completedJobIds directly.
      return !completedJobIds.has(trimmed) && trimmed !== '';
    });

    // Always keep at least one empty input
    set({ inputs: remainingInputs.length > 0 ? remainingInputs : [''] });
  },

  setRunning: (running) => set({ isRunning: running }),
  setVerboseMode: (verbose) => {
    set({ verboseMode: verbose });
    persistLiveModes(get().currentModes, verbose);
  },
  setModes: (modes) => {
    // Summarize requires transcript. Clear it automatically if transcript
    // was unchecked so the UI can never enter an inconsistent state.
    const nextModes = modes.transcript ? modes : { ...modes, summarize: false };
    set({ currentModes: nextModes });
    persistLiveModes(nextModes, get().verboseMode);
  },

  addJob: (itemId) => {
    const { jobs, currentModes } = get();
    if (jobs.has(itemId)) return;

    const newJob: JobItem = {
      itemId,
      displayName: '',
      status: 'queued',
      artifacts: {
        video: currentModes.video ? 'queued' : 'not_requested',
        audio: currentModes.audio ? 'queued' : 'not_requested',
        transcript: currentModes.transcript ? 'queued' : 'not_requested',
        summary: currentModes.summarize ? 'queued' : 'not_requested',
      },
    };

    set({ jobs: new Map(jobs).set(itemId, newJob) });
  },

  updateJobStatus: (itemId, status) => {
    const { jobs } = get();
    const job = jobs.get(itemId);
    if (!job) return;

    const updated = { ...job, status };
    set({ jobs: new Map(jobs).set(itemId, updated) });
  },

  updateArtifactStatus: (itemId, artifact, status) => {
    const { jobs } = get();
    const job = jobs.get(itemId);
    if (!job) return;

    const updated = {
      ...job,
      artifacts: {
        ...job.artifacts,
        [artifact]: status as ArtifactStatus,
      },
    };
    set({ jobs: new Map(jobs).set(itemId, updated) });
  },

  setFilestemOverride: (itemId, filestem) => {
    const { jobs } = get();
    const job = jobs.get(itemId);
    if (!job) return;

    const updated = { ...job, displayName: filestem };
    set({ jobs: new Map(jobs).set(itemId, updated) });
  },

  setStatusTotal: (total) => set({ statusTotal: total }),

  removeJob: (itemId) => {
    const { jobs } = get();
    const newJobs = new Map(jobs);
    newJobs.delete(itemId);
    set({ jobs: newJobs });
  },

  clearCompletedJobs: () => {
    const { jobs } = get();
    const filtered = new Map<string, JobItem>();

    for (const [id, job] of jobs) {
      // Keep only jobs that are still queued or running
      if (job.status === 'queued' || job.status === 'running') {
        filtered.set(id, job);
      }
    }

    set({ jobs: filtered });
  },

  // "Clear All" — a total cull. Drops every job row (including hidden
  // channel-container placeholders that have no ⛔ of their own) AND the entire
  // pending queue in one atomic update. The caller cancels the running backend;
  // its backend_exited then resets isRunning and finds an empty queue. No
  // confirmation by design — matches the app's no-prompt philosophy.
  clearAllJobs: () => set({ jobs: new Map(), jobQueue: [] }),

  reset: () => set({
    jobs: new Map(),
    clipTimestamps: [],
    statusTotal: 0,
    manualCleanInputs: [],
    skipInputs: [],
  }),

  setShowEpisodeLimit: (show) => set({ showEpisodeLimit: show }),
  setEpisodeLimit: (limit) => set({ episodeLimit: limit }),

  addManualCleanInput: (path) => set((state) => ({
    manualCleanInputs: [...state.manualCleanInputs, path],
  })),

  addSkipInput: (path) => set((state) => ({
    skipInputs: [...state.skipInputs, path],
  })),

  enqueueJobs: (units) =>
    set((state) => ({ jobQueue: [...state.jobQueue, ...units] })),

  // Used by the dispatcher's retry path when start_job failed because the
  // previous backend hadn't finished exiting: the unit goes back to the FRONT
  // so the next backend_exited dispatch retries it before newer units.
  requeueJobAtFront: (unit) =>
    set((state) => ({ jobQueue: [unit, ...state.jobQueue] })),

  dequeueJob: () => {
    const { jobQueue } = get();
    if (jobQueue.length === 0) return undefined;
    const [next, ...rest] = jobQueue;
    set({ jobQueue: rest });
    return next;
  },

  clearJobQueue: () => set({ jobQueue: [] }),
}));
