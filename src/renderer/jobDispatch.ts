import type { JobConfig } from '@/shared/types';
import { useJobStore, type QueuedJob } from './store/jobStore';
import { useSettingsStore } from './store/settingsStore';
import { useLogStore } from './store/logStore';
import { getDesktopAPI } from './desktopApi';
import { sanitizeUrl } from './utils/youtube';

// The unified job pipeline (manual Go AND Firefox intake both flow through it).
//
// Model: inputs vs jobs. On the GO signal, the settings in effect at that exact
// instant are frozen into a per-job snapshot (config.ini is copied; the rest of
// the per-submission state is captured into the JobConfig payload). Each unique
// input becomes its own immutable job, appended to the queue, and the backend
// runs them one process at a time. Changing settings afterwards — or pressing
// Go again — can never mutate a job that already exists.

// Dispatch the next queued job, if any and if the backend is idle. The shell
// runs one backend process at a time, so we only fire when no process is active:
// on the initial submission (when idle) and on each `backend_exited`.
export function dispatchNext(): void {
  const store = useJobStore.getState();
  if (store.isRunning) return; // a process is active — wait for backend_exited
  const unit = store.dequeueJob();
  if (!unit) return;

  // Mark running optimistically (synchronously) so a re-entrant dispatchNext —
  // e.g. a Go press landing in the same tick — sees it and defers instead of
  // double-dispatching.
  store.setRunning(true);
  void getDesktopAPI()
    .startJob(unit.config)
    .catch((err) => {
      const text = String(err);
      const log = useLogStore.getState();
      if (text.includes('Backend already running')) {
        // The previous backend's process slot hadn't cleared within start_job's
        // grace window (exact Err string from the Tauri shell — keep in sync
        // with main.rs). Put the unit back at the FRONT and leave isRunning
        // true: that previous process IS still alive, and its `backend_exited`
        // will reset isRunning and re-trigger dispatchNext, which picks this
        // unit up first.
        useJobStore.getState().requeueJobAtFront(unit);
        return;
      }
      // Hard spawn failure (missing exe, OS error). It would fail for every
      // queued job too, so abort the whole queue rather than spin on errors;
      // the inputs are still in the field for a manual retry once fixed.
      const dropped = useJobStore.getState().jobQueue.length;
      useJobStore.getState().clearJobQueue();
      useJobStore.getState().setRunning(false);
      const msg =
        `Failed to start backend: ${text}` +
        (dropped > 0 ? ` — cleared ${dropped} queued job(s).` : '');
      log.addLog(msg, true);
      void getDesktopAPI().logMessage(msg);
    });
}

// The GO signal. Reads the current inputs + settings, freezes config once per
// input, turns each NEW unique input into a frozen QueuedJob, and kicks the
// dispatcher. `timeframe` is the (channel) timeframe string composed in
// InputSection; Firefox intakes pass none.
//
// Dedup: an input is rejected if a job with that sanitized id already exists in
// the table in ANY state (queued/running/completed/failed), or it repeats
// earlier in this same press. To re-run an input, remove its job from the table
// first. A press with no new uniques is a harmless no-op (so Go can stay
// always-enabled and button-mashing does nothing).
export async function submitInputs(timeframe?: string): Promise<void> {
  const job = useJobStore.getState();
  const { settings } = useSettingsStore.getState();
  const log = useLogStore.getState();

  const valid = job.inputs
    .map((input, index) => ({ input: input.trim(), index }))
    .filter((entry) => entry.input !== '');
  if (valid.length === 0) return;

  const existing = job.jobs;
  const seen = new Set<string>();
  const fresh: { itemId: string; index: number }[] = [];
  for (const { input, index } of valid) {
    const itemId = sanitizeUrl(input);
    if (existing.has(itemId) || seen.has(itemId)) {
      const why = seen.has(itemId) ? 'repeated in this submission' : 'already a job';
      const msg = `Skipped duplicate input (${why}): ${itemId}`;
      log.addLog(msg);
      void getDesktopAPI().logMessage(msg);
      continue;
    }
    seen.add(itemId);
    fresh.push({ itemId, index });
  }
  if (fresh.length === 0) return;

  // Reserve the UI jobs synchronously NOW, before the async freeze. Everything
  // up to the first await runs atomically on JS's single thread, so a second Go
  // press landing during the freeze will see these in `jobs` and dedup against
  // them — without this, rapid double-presses would each pass dedup and create
  // duplicate jobs. addJob captures the GO-instant modes for the placeholder row.
  const addJob = useJobStore.getState().addJob;
  fresh.forEach(({ itemId }) => addJob(itemId));

  // Capture the transient per-submission state at the GO instant (before the
  // async freeze), so a checkbox toggle landing mid-freeze can't bleed into
  // this submission's jobs. Summarize+Fast = skip LLM cleaning; for URL inputs
  // force captions too so whisper isn't run only to discard its output.
  const fastSummarize =
    job.currentModes.summarize && settings.default_summarize_mode === 'fast';
  const frozen = {
    video: job.currentModes.video,
    audio: job.currentModes.audio,
    transcript: job.currentModes.transcript,
    summarize: job.currentModes.summarize,
    verbose: job.verboseMode,
    model: settings.default_model || undefined,
    transcriptSource: fastSummarize ? 'captions' : settings.default_transcript_source,
    autoClean: fastSummarize ? 'off' : undefined,
    keep: settings.default_keep,
    episodeLimit: job.episodeLimit || undefined,
    manualCleanInputs: job.manualCleanInputs.length > 0 ? job.manualCleanInputs : undefined,
    skipInputs: job.skipInputs.length > 0 ? job.skipInputs : undefined,
    clipTimestamps: job.clipTimestamps,
  };

  // Freeze config.ini once PER input (one process per input, and the shell
  // deletes each snapshot when its process exits, so the jobs must not share a
  // file). All snapshots in this press capture the same instant.
  let snapshots: string[];
  try {
    snapshots = await Promise.all(fresh.map(() => getDesktopAPI().freezeConfig()));
  } catch (err) {
    // Roll back the reserved jobs so a failed freeze doesn't leave permanent
    // placeholder rows (and so the inputs can be retried cleanly).
    const removeJob = useJobStore.getState().removeJob;
    fresh.forEach(({ itemId }) => removeJob(itemId));
    const msg = `Failed to freeze config for submission: ${String(err)}`;
    log.addLog(msg, true);
    void getDesktopAPI().logMessage(msg);
    return;
  }

  const units: QueuedJob[] = fresh.map(({ itemId, index }, i) => {
    const clips = (frozen.clipTimestamps[index] || []).filter((c) => c.start || c.end);
    const config: JobConfig = {
      inputs: [itemId],
      video: frozen.video,
      audio: frozen.audio,
      transcript: frozen.transcript,
      summarize: frozen.summarize,
      verbose: frozen.verbose,
      timeframe,
      model: frozen.model,
      transcript_source: frozen.transcriptSource,
      auto_clean_transcript: frozen.autoClean,
      keep: frozen.keep,
      clipTimestamps: [clips],
      episodeLimit: frozen.episodeLimit,
      manualCleanInputs: frozen.manualCleanInputs,
      skipInputs: frozen.skipInputs,
      config_snapshot_path: snapshots[i],
    };
    return { itemId, config };
  });

  // UI jobs were reserved before the freeze; enqueue the frozen units and kick
  // the dispatcher.
  useJobStore.getState().enqueueJobs(units);
  dispatchNext();
}
