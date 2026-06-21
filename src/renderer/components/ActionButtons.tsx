import { useJobStore } from '../store/jobStore';
import { useThemeStore, themes } from '../store/themeStore';
import { getDesktopAPI } from '../desktopApi';
import { submitInputs } from '../jobDispatch';

export function ActionButtons({ timeframe }: { timeframe?: string }) {
  const isRunning = useJobStore((s) => s.isRunning);
  const queueLength = useJobStore((s) => s.jobQueue.length);
  const currentTheme = useThemeStore((s) => s.theme);
  const themeColors = themes[currentTheme];

  const handleGo = () => {
    // Always enabled. submitInputs dedups against existing jobs and is a no-op
    // when there are no new unique inputs, so button-mashing does nothing. New
    // uniques are frozen at this instant and appended to the queue to auto-run
    // after whatever is already running/queued.
    void submitInputs(timeframe);
  };

  const handleStop = () => {
    // Dumb full kill: clear the queue so nothing auto-starts, then cancel the
    // running backend (and all its child processing). Not robust to resuming or
    // restarting unfinished jobs — that's deliberately deferred. It just stops
    // everything.
    useJobStore.getState().clearJobQueue();
    getDesktopAPI().cancelJob();
  };

  // Stop is live whenever there's anything to stop: a running process or pending
  // queued jobs. Go is always live (see handleGo).
  const canStop = isRunning || queueLength > 0;

  return (
    <div className="flex gap-2">
      <button
        onClick={handleGo}
        className="px-4 py-2 text-white rounded-md transition-transform duration-[80ms] hover:brightness-110 active:scale-95 active:brightness-90"
        style={{
          backgroundColor: themeColors.buttonPrimary,
          fontSize: '1.1rem',
        }}
      >
        Go
      </button>
      <button
        onClick={handleStop}
        disabled={!canStop}
        className="px-4 py-2 text-white rounded-md transition-transform duration-[80ms] disabled:cursor-not-allowed hover:brightness-110 active:scale-95 active:brightness-90"
        style={{
          backgroundColor: canStop ? themeColors.buttonDanger : themeColors.buttonDisabled,
          fontSize: '1.1rem',
        }}
      >
        Stop
      </button>
    </div>
  );
}
