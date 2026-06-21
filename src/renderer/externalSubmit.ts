import type { ExternalJobRequest, ExternalJobPreset, JobConfig } from '@/shared/types';
import { useJobStore } from './store/jobStore';
import { useSettingsStore } from './store/settingsStore';
import { useLogStore } from './store/logStore';
import { getDesktopAPI } from './desktopApi';
import { sanitizeUrl } from './utils/youtube';

const PRESET_LABELS: Record<ExternalJobPreset, string> = {
  archive_video: 'Archive video',
  save_audio: 'Save audio',
  save_transcript: 'Save transcript',
  summarize: 'Summarize',
};

function labelFor(preset: string): string {
  return (PRESET_LABELS as Record<string, string>)[preset] ?? preset;
}

// Submit a Firefox intake as a fresh batch of one. Mirrors the manual Go flow:
// reset, set modes, set inputs (single URL), add job, set running, startJob.
// Used both by the idle-path in handleExternalJobRequest and by the
// backend_exited drain hook in useBackendEvents — any change to the submission
// shape (JobConfig fields, summarize flag, etc.) must stay in sync here.
//
// Inputs-field handling: ensures this URL is represented in the input field
// without clobbering whatever else is there (user typing, other pending
// Firefox URLs the user saw arrive earlier). Existing inputs stay; the
// URL is appended only if not already present.
export async function startExternalJobNow(request: ExternalJobRequest): Promise<void> {
  const jobStore = useJobStore.getState();
  const settingsStore = useSettingsStore.getState();
  const logStore = useLogStore.getState();

  const { verboseMode } = jobStore;
  const { settings } = settingsStore;
  const label = labelFor(request.preset);

  jobStore.reset();
  jobStore.setModes(request.modes);

  const sanitizedUrl = sanitizeUrl(request.url);
  const currentInputs = useJobStore.getState().inputs;
  const alreadyInField = currentInputs.some((i) => i.trim() === sanitizedUrl);
  if (!alreadyInField) {
    const kept = currentInputs.filter((input) => input.trim() !== '');
    kept.push(sanitizedUrl);
    kept.push('');
    jobStore.setInputs(kept);
  }

  const transcriptSource =
    request.overrides?.transcript_source ?? settings.default_transcript_source;

  // The backend owns the clean → summarize chain, so summarize just rides
  // along in JobConfig — no frontend-side state machine needed.
  const config: JobConfig = {
    inputs: [sanitizedUrl],
    video: request.modes.video,
    audio: request.modes.audio,
    transcript: request.modes.transcript,
    summarize: request.modes.summarize,
    verbose: verboseMode,
    model: settings.default_model || undefined,
    transcript_source: transcriptSource,
    keep: settings.default_keep,
    clipTimestamps: [[]],
  };

  jobStore.addJob(sanitizedUrl);
  jobStore.setRunning(true);

  const msg = `Received from Firefox (${label}): ${request.url}`;
  logStore.addLog(msg);
  // Await before startJob so the Tauri command dispatcher writes this line
  // ahead of the backend's "Starting backend (rust):..." entry.
  await getDesktopAPI().logMessage(msg);

  try {
    await getDesktopAPI().startJob(config);
  } catch (err) {
    const errText = String(err);
    if (errText.includes('Backend already running')) {
      // The previous backend's process slot hadn't cleared within
      // start_job's grace window (exact Err string from the Tauri shell —
      // keep in sync with main.rs). The slot WILL clear, and the exit
      // monitor emits `backend_exited` when it does, so put the request
      // back at the front of the queue for that drain to retry. isRunning
      // intentionally stays true: intakes arriving meanwhile keep queueing
      // instead of misrouting to the idle/composing paths.
      jobStore.requeueExternalJobAtFront(request);
      const failMsg = `${label} intake (${request.url}) couldn't start — previous backend still shutting down; re-queued to retry as soon as it exits.`;
      logStore.addLog(failMsg);
      void getDesktopAPI().logMessage(failMsg);
    } else {
      // Hard spawn failure (missing exe, OS error). Don't re-queue —
      // nothing would retry it. The URL is still in the input field, so a
      // manual Go can resubmit once the cause is fixed.
      jobStore.setRunning(false);
      const failMsg = `${label} intake (${request.url}) failed to start: ${errText}`;
      logStore.addLog(failMsg, true);
      void getDesktopAPI().logMessage(failMsg);
    }
  }
}

export function handleExternalJobRequest(request: ExternalJobRequest): void {
  const jobStore = useJobStore.getState();
  const logStore = useLogStore.getState();

  const { isRunning, inputs, pendingExternalJobs } = jobStore;
  const hasNonEmptyInput = inputs.some((input) => input.trim() !== '');
  // A non-empty queue means a drain is pending (e.g. the previous backend is
  // exiting and backend_exited hasn't fired yet) — treat that as busy even if
  // isRunning momentarily reads false, otherwise an intake landing in that
  // gap would misroute to the composing path below and never auto-start.
  const queueBusy = isRunning || pendingExternalJobs.length > 0;
  const label = labelFor(request.preset);

  if (!queueBusy && !hasNonEmptyInput) {
    void startExternalJobNow(request);
    return;
  }

  // Otherwise, append the URL to the input field for visibility.
  const kept = inputs.filter((input) => input.trim() !== '');
  kept.push(request.url);
  kept.push('');
  jobStore.setInputs(kept);

  if (queueBusy) {
    // Active batch (or a drain already pending) — enqueue for auto-start.
    // The next backend_exited event will pop this and call
    // startExternalJobNow.
    jobStore.enqueueExternalJob(request);
    const msg = `Received from Firefox (${label}): ${request.url} — queued, will auto-start when current batch finishes.`;
    logStore.addLog(msg);
    void getDesktopAPI().logMessage(msg);
  } else {
    // Idle but the user is composing a batch (has text in the input field).
    // DON'T enqueue — the user is about to press Go, which will include this
    // URL in their batch via the standard handleGo path. Enqueuing here would
    // cause the drain to re-submit it after their batch finishes. The user
    // is actively engaged with the UI; respect their intent and let them
    // press Go when ready.
    const msg = `Received from Firefox (${label}): ${request.url} — added to input field.`;
    logStore.addLog(msg);
    void getDesktopAPI().logMessage(msg);
  }
}
