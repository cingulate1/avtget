import { useState } from 'react';
import { useJobStore } from '../store/jobStore';
import { useThemeStore, themes } from '../store/themeStore';
import { SettingsDialog } from './SettingsDialog';
import { ClipTimestampsDialog } from './ClipTimestampsDialog';

export function OptionsRow() {
  const currentModes = useJobStore((s) => s.currentModes);
  const setModes = useJobStore((s) => s.setModes);
  const verboseMode = useJobStore((s) => s.verboseMode);
  const setVerboseMode = useJobStore((s) => s.setVerboseMode);
  const inputs = useJobStore((s) => s.inputs);
  const clipTimestamps = useJobStore((s) => s.clipTimestamps);
  const setClipTimestamps = useJobStore((s) => s.setClipTimestamps);
  const [showSettings, setShowSettings] = useState(false);
  const currentTheme = useThemeStore((s) => s.theme);
  const themeColors = themes[currentTheme];

  return (
    <div className="flex items-center gap-4">
      <label className="flex items-center gap-2">
        <input
          type="checkbox"
          checked={currentModes.video}
          onChange={(e) => setModes({ ...currentModes, video: e.target.checked })}
          className="w-4 h-4 transition-all duration-[400ms]"
          style={{ accentColor: themeColors.buttonPrimary }}
        />
        <span
          className="text-sm font-medium transition-all duration-[400ms]"
          style={{ color: themeColors.text }}
        >
          Video
        </span>
      </label>

      <label className="flex items-center gap-2">
        <input
          type="checkbox"
          checked={currentModes.audio}
          onChange={(e) => setModes({ ...currentModes, audio: e.target.checked })}
          className="w-4 h-4 transition-all duration-[400ms]"
          style={{ accentColor: themeColors.buttonPrimary }}
        />
        <span
          className="text-sm font-medium transition-all duration-[400ms]"
          style={{ color: themeColors.text }}
        >
          Audio
        </span>
      </label>

      <label className="flex items-center gap-2">
        <input
          type="checkbox"
          checked={currentModes.transcript}
          onChange={(e) => setModes({ ...currentModes, transcript: e.target.checked })}
          className="w-4 h-4 transition-all duration-[400ms]"
          style={{ accentColor: themeColors.buttonPrimary }}
        />
        <span
          className="text-sm font-medium transition-all duration-[400ms]"
          style={{ color: themeColors.text }}
        >
          Transcript
        </span>
      </label>

      <label
        className="flex items-center gap-2"
        style={{ opacity: currentModes.transcript ? 1 : 0.45 }}
      >
        <input
          type="checkbox"
          checked={currentModes.summarize}
          disabled={!currentModes.transcript}
          onChange={(e) => setModes({ ...currentModes, summarize: e.target.checked })}
          className="w-4 h-4 transition-all duration-[400ms] disabled:cursor-not-allowed"
          style={{ accentColor: themeColors.buttonPrimary }}
        />
        <span
          className="text-sm font-medium transition-all duration-[400ms]"
          style={{ color: themeColors.text }}
        >
          Summarize
        </span>
      </label>

      <label className="flex items-center gap-2">
        <input
          type="checkbox"
          checked={verboseMode}
          onChange={(e) => setVerboseMode(e.target.checked)}
          className="w-4 h-4 transition-all duration-[400ms]"
          style={{ accentColor: themeColors.buttonPrimary }}
        />
        <span
          className="text-sm font-medium transition-all duration-[400ms]"
          style={{ color: themeColors.text }}
        >
          Verbose
        </span>
      </label>

      {/* Clip Timestamps button */}
      <ClipTimestampsDialog
        inputs={inputs}
        clipTimestamps={clipTimestamps}
        setClipTimestamps={setClipTimestamps}
      />

      {/* Spacer to push Settings to the right */}
      <div className="flex-1" />

      <button
        onClick={() => setShowSettings(true)}
        className="px-4 py-2 text-white rounded-md transition-transform duration-[80ms] hover:brightness-110 active:scale-95 active:brightness-90"
        style={{ backgroundColor: themeColors.buttonPrimary }}
      >
        Settings
      </button>

      {showSettings && <SettingsDialog onClose={() => setShowSettings(false)} />}
    </div>
  );
}

