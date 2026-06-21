import { useJobStore } from '../store/jobStore';
import { useLogStore } from '../store/logStore';
import { useSettingsStore } from '../store/settingsStore';
import { useThemeStore, themes } from '../store/themeStore';
import { getDesktopAPI } from '../desktopApi';
import type { JobConfig } from '@/shared/types';

export function ActionButtons({ inputs, timeframe }: { inputs: string[]; timeframe?: string }) {
  const desktopAPI = getDesktopAPI();
  const isRunning = useJobStore((s) => s.isRunning);
  const setRunning = useJobStore((s) => s.setRunning);
  const reset = useJobStore((s) => s.reset);
  const currentModes = useJobStore((s) => s.currentModes);
  const verboseMode = useJobStore((s) => s.verboseMode);
  const clipTimestamps = useJobStore((s) => s.clipTimestamps);
  const episodeLimit = useJobStore((s) => s.episodeLimit);
  const manualCleanInputs = useJobStore((s) => s.manualCleanInputs);
  const skipInputs = useJobStore((s) => s.skipInputs);
  const settings = useSettingsStore((s) => s.settings);
  const currentTheme = useThemeStore((s) => s.theme);
  const themeColors = themes[currentTheme];

  const handleGo = () => {
    // Filter out empty inputs
    const validInputs = inputs.filter((input) => input.trim() !== '');
    if (validInputs.length === 0) return;

    // Filter clip timestamps to match valid inputs and remove empty ranges
    const validClipTimestamps = validInputs.map((_, i) => {
      const clips = clipTimestamps[i] || [];
      return clips.filter(c => c.start || c.end);
    });

    // Summarize+Fast = "skip the LLM cleaning step." For URL inputs we also
    // force transcript_source=captions so we don't kick off whisper just to
    // throw the cleaned output away. .txt local input becomes a direct
    // summarize. Local audio/video still runs whisper (no captions exist for
    // them) but skips cleaning.
    const fastSummarize =
      currentModes.summarize && settings.default_summarize_mode === 'fast';

    // Build job config. `summarize` rides along with the rest so the backend
    // orchestrator can drive clean → summarize sequentially per item.
    const config: JobConfig = {
      inputs: validInputs,
      video: currentModes.video,
      audio: currentModes.audio,
      transcript: currentModes.transcript,
      summarize: currentModes.summarize,
      verbose: verboseMode,
      timeframe,
      model: settings.default_model || undefined,
      transcript_source: fastSummarize ? 'captions' : settings.default_transcript_source,
      auto_clean_transcript: fastSummarize ? 'off' : undefined,
      keep: settings.default_keep,
      clipTimestamps: validClipTimestamps,
      episodeLimit: episodeLimit || undefined,
      // Include transcript file handling flags
      manualCleanInputs: manualCleanInputs.length > 0 ? manualCleanInputs : undefined,
      skipInputs: skipInputs.length > 0 ? skipInputs : undefined,
    };

    reset();

    // Optimistically add jobs to the UI. Inputs are already sanitized by the
    // jobStore's setInputs, so they match what the backend will echo as item_ids.
    validInputs.forEach((input) => {
      useJobStore.getState().addJob(input);
    });

    setRunning(true);
    void desktopAPI.startJob(config).catch((err) => {
      // Without this catch the UI would stay stuck on "running" with no
      // backend process. Likeliest cause is the previous backend still
      // shutting down (start_job gives up after 2 s) — pressing Go again
      // once it has exited will succeed.
      useJobStore.getState().setRunning(false);
      const msg = `Failed to start job: ${err}`;
      useLogStore.getState().addLog(msg, true);
      void desktopAPI.logMessage(msg);
    });
  };

  const handleStop = () => {
    // Stop means stop everything — cancel the running batch AND discard any
    // Firefox-extension intakes that were waiting to auto-start. Clearing
    // before the cancelled backend exits ensures the backend_exited drain
    // in useBackendEvents sees an empty pending queue and does nothing.
    useJobStore.getState().clearPendingExternalJobs();
    desktopAPI.cancelJob();
  };

  const validInputs = inputs.filter((input) => input.trim() !== '');
  const canStart = validInputs.length > 0 && !isRunning;

  return (
    <div className="flex gap-2">
      <button
        onClick={handleGo}
        disabled={!canStart}
        className="px-4 py-2 text-white rounded-md transition-transform duration-[80ms] disabled:cursor-not-allowed hover:brightness-110 active:scale-95 active:brightness-90"
        style={{
          backgroundColor: canStart ? themeColors.buttonPrimary : themeColors.buttonDisabled,
          fontSize: '1.1rem',
        }}
      >
        Go
      </button>
      <button
        onClick={handleStop}
        disabled={!isRunning}
        className="px-4 py-2 text-white rounded-md transition-transform duration-[80ms] disabled:cursor-not-allowed hover:brightness-110 active:scale-95 active:brightness-90"
        style={{
          backgroundColor: isRunning ? themeColors.buttonDanger : themeColors.buttonDisabled,
          fontSize: '1.1rem',
        }}
      >
        Stop
      </button>
    </div>
  );
}
