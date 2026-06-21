import { useEffect } from 'react';
import { useSettingsStore } from './store/settingsStore';
import { useJobStore } from './store/jobStore';
import { useThemeStore, themes } from './store/themeStore';
import { useBackendEvents } from './hooks/useBackendEvents';
import { useExternalJobRequests } from './hooks/useExternalJobRequests';
import { InputSection } from './components/InputSection';
import { OptionsRow } from './components/OptionsRow';
import { JobTable } from './components/JobTable';
import { LogPanel } from './components/LogPanel';
import { ThemeSelector } from './components/ThemeSelector';
import { getDesktopAPI } from './desktopApi';

export default function App() {
  const desktopAPI = getDesktopAPI();
  const loadSettings = useSettingsStore((s) => s.loadSettings);
  const isLoaded = useSettingsStore((s) => s.isLoaded);
  const settings = useSettingsStore((s) => s.settings);
  const setModes = useJobStore((s) => s.setModes);
  const setVerboseMode = useJobStore((s) => s.setVerboseMode);
  const currentTheme = useThemeStore((s) => s.theme);
  const themeColors = themes[currentTheme];
  const inputs = useJobStore((s) => s.inputs);

  // Disable default context menu app-wide
  useEffect(() => {
    const suppress = (e: MouseEvent) => e.preventDefault();
    document.addEventListener('contextmenu', suppress);
    return () => document.removeEventListener('contextmenu', suppress);
  }, []);

  // Load settings on startup
  useEffect(() => {
    loadSettings();
  }, [loadSettings]);

  // Ctrl+mousewheel zoom
  useEffect(() => {
    const handleWheel = (e: WheelEvent) => {
      if (e.ctrlKey) {
        e.preventDefault();
        const currentZoom = desktopAPI.getZoomLevel();
        const delta = e.deltaY > 0 ? -0.5 : 0.5;
        desktopAPI.setZoomLevel(currentZoom + delta);
      }
    };

    window.addEventListener('wheel', handleWheel, { passive: false });
    return () => window.removeEventListener('wheel', handleWheel);
  }, []);

  // Save preset menu handler
  useEffect(() => {
    const unsubscribe = desktopAPI.onSavePresetRequest(async () => {
      // Gather current state
      const preset = {
        inputs,
        modes: useJobStore.getState().currentModes,
        verboseMode: useJobStore.getState().verboseMode,
        clipTimestamps: useJobStore.getState().clipTimestamps,
        episodeLimit: useJobStore.getState().episodeLimit,
        settings,
        theme: currentTheme,
      };

      // Show save dialog
      const result = await desktopAPI.showSaveDialog({
        title: 'Save Preset',
        defaultPath: 'avtget-preset.json',
        filters: [{ name: 'JSON', extensions: ['json'] }],
      });

      if (result.filePath) {
        await desktopAPI.writeTextFile(result.filePath, JSON.stringify(preset, null, 2));
      }
    });

    return unsubscribe;
  }, [inputs, settings, currentTheme]);

  // Apply default settings to UI - ONLY on initial load
  useEffect(() => {
    if (isLoaded) {
      setModes({
        video: settings.default_video,
        audio: settings.default_audio,
        transcript: settings.default_transcript,
        summarize: settings.default_transcript && settings.default_summarize,
      });
      setVerboseMode(settings.default_verbose);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isLoaded]);

  // Subscribe to backend events
  useBackendEvents();
  // Subscribe to external job requests from the Firefox extension bridge
  useExternalJobRequests();

  if (!isLoaded) {
    return (
      <div
        className="h-screen flex items-center justify-center transition-all duration-[400ms]"
        style={{ backgroundColor: themeColors.bg }}
      >
        <div style={{ color: themeColors.textMuted }}>Loading...</div>
      </div>
    );
  }

  return (
    <div
      className="h-screen flex flex-col p-4 pt-8 gap-4 transition-all duration-[400ms] relative overflow-y-auto"
      style={{ backgroundColor: themeColors.bg }}
    >
      {/* Theme selector in top-right corner */}
      <div className="absolute top-2 right-3 z-10">
        <ThemeSelector />
      </div>

      <div
        className="rounded-lg shadow p-4 transition-all duration-[400ms] flex-shrink-0"
        style={{
          backgroundColor: themeColors.cardBg,
          borderColor: themeColors.border,
        }}
      >
        <InputSection />
      </div>

      <div
        className="rounded-lg shadow p-4 transition-all duration-[400ms] flex-shrink-0"
        style={{
          backgroundColor: themeColors.cardBg,
          borderColor: themeColors.border,
        }}
      >
        <OptionsRow />
      </div>

      <div
        className="flex-1 rounded-lg shadow p-4 min-h-[120px] transition-all duration-[400ms]"
        style={{
          backgroundColor: themeColors.cardBg,
          borderColor: themeColors.border,
        }}
      >
        <JobTable />
      </div>

      <div className="flex-shrink-0">
        <LogPanel />
      </div>
    </div>
  );
}
