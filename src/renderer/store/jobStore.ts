import { create } from 'zustand';
import type { JobItem, JobStatus, ArtifactStatus, ExternalJobRequest } from '@/shared/types';
import { sanitizeUrl } from '../utils/youtube';

interface ProcessingModes {
  video: boolean;
  audio: boolean;
  transcript: boolean;
  summarize: boolean;
}

export interface ClipRange {
  start: string;
  end: string;
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
  // Firefox-extension intakes that arrived while Avtget was busy or the input
  // fields were populated. Drained one-at-a-time on each `backend_exited`
  // event, at which point the shell's process slot is guaranteed free — see
  // useBackendEvents.ts. Survives `reset()` so a fresh batch kicked off by
  // the drain doesn't lose the rest of the queue.
  pendingExternalJobs: ExternalJobRequest[];

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
  reset: () => void;
  setShowEpisodeLimit: (show: boolean) => void;
  setEpisodeLimit: (limit: number) => void;
  addManualCleanInput: (path: string) => void;
  addSkipInput: (path: string) => void;
  enqueueExternalJob: (request: ExternalJobRequest) => void;
  requeueExternalJobAtFront: (request: ExternalJobRequest) => void;
  drainNextExternalJob: () => ExternalJobRequest | undefined;
  clearPendingExternalJobs: () => void;
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
  pendingExternalJobs: [],

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
  setVerboseMode: (verbose) => set({ verboseMode: verbose }),
  setModes: (modes) =>
    set({
      // Summarize requires transcript. Clear it automatically if transcript
      // was unchecked so the UI can never enter an inconsistent state.
      currentModes: modes.transcript ? modes : { ...modes, summarize: false },
    }),

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

  enqueueExternalJob: (request) =>
    set((state) => ({ pendingExternalJobs: [...state.pendingExternalJobs, request] })),

  // Used by the intake-retry path when start_job failed because the previous
  // backend hadn't finished exiting: the request goes back to the FRONT so
  // the next backend_exited drain retries it before newer intakes.
  requeueExternalJobAtFront: (request) =>
    set((state) => ({ pendingExternalJobs: [request, ...state.pendingExternalJobs] })),

  drainNextExternalJob: () => {
    const { pendingExternalJobs } = get();
    if (pendingExternalJobs.length === 0) return undefined;
    const [next, ...rest] = pendingExternalJobs;
    set({ pendingExternalJobs: rest });
    return next;
  },

  clearPendingExternalJobs: () => set({ pendingExternalJobs: [] }),
}));
