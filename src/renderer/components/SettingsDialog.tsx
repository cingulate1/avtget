import { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useSettingsStore } from '../store/settingsStore';
import { getDesktopAPI } from '../desktopApi';
import type { Settings } from '@/shared/types';

interface SettingsDialogProps {
  onClose: () => void;
}

export function SettingsDialog({ onClose }: SettingsDialogProps) {
  const desktopAPI = getDesktopAPI();
  const settings = useSettingsStore((s) => s.settings);
  const saveSettings = useSettingsStore((s) => s.saveSettings);
  const [localSettings, setLocalSettings] = useState<Settings>(settings);
  const [ollamaAvailable, setOllamaAvailable] = useState<boolean | null>(null);

  useEffect(() => {
    setLocalSettings(settings);
  }, [settings]);

  useEffect(() => {
    invoke<{ installed: boolean; running: boolean }>('check_ollama_available')
      .then((status) => setOllamaAvailable(status.installed))
      .catch(() => setOllamaAvailable(false));
  }, []);

  const handleSave = async () => {
    // Validate that storage and temp directories are not the same
    if (
      localSettings.storage_directory &&
      localSettings.temp_directory &&
      localSettings.storage_directory.trim().toLowerCase() === localSettings.temp_directory.trim().toLowerCase()
    ) {
      const response = await desktopAPI.showMessageBox({
        type: 'warning',
        title: 'Directory Warning',
        message: 'Storage and Temp directories are identical.',
        detail: 'It is highly recommended to keep these directories separate to avoid file locking issues and accidental data loss.\n\nAre you sure you want to proceed?',
        buttons: ['Cancel', 'Proceed Anyway'],
        defaultId: 0,
        cancelId: 0,
      });

      if (response === 0) {
        return; // User cancelled
      }
    }

    await saveSettings(localSettings);
    onClose();
  };

  const handleBrowse = async (key: keyof Settings) => {
    const isDirectory = key.includes('directory');
    const result = await desktopAPI.showOpenDialog({
      properties: isDirectory ? ['openDirectory'] : ['openFile'],
    });

    if (result) {
      setLocalSettings({ ...localSettings, [key]: result });
    }
  };

  return (
    <div className="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50">
      <div className="bg-white rounded-lg p-4 w-[960px] max-w-[96vw]">
        <h2 className="text-lg font-bold mb-3">Settings</h2>

        <div className="grid grid-cols-2 gap-4">
          <div className="space-y-2">
            <div>
              <label className="block text-xs font-medium mb-0.5">Storage Directory</label>
              <div className="flex gap-1.5">
                <input
                  type="text"
                  value={localSettings.storage_directory}
                  onChange={(e) => setLocalSettings({ ...localSettings, storage_directory: e.target.value })}
                  className="flex-1 px-2 py-1 text-sm border border-gray-300 rounded-md"
                />
                <button
                  onClick={() => handleBrowse('storage_directory')}
                  className="px-2 py-1 text-xs bg-gray-600 text-white rounded-md hover:bg-gray-700"
                >
                  Browse
                </button>
              </div>
            </div>

            <div>
              <label className="block text-xs font-medium mb-0.5">Temp Directory</label>
              <div className="flex gap-1.5">
                <input
                  type="text"
                  value={localSettings.temp_directory}
                  onChange={(e) => setLocalSettings({ ...localSettings, temp_directory: e.target.value })}
                  className="flex-1 px-2 py-1 text-sm border border-gray-300 rounded-md"
                />
                <button
                  onClick={() => handleBrowse('temp_directory')}
                  className="px-2 py-1 text-xs bg-gray-600 text-white rounded-md hover:bg-gray-700"
                >
                  Browse
                </button>
              </div>
            </div>

            <div>
              <label className="block text-xs font-medium mb-0.5">Filename Template</label>
              <input
                type="text"
                value={localSettings.filename_template}
                onChange={(e) => setLocalSettings({ ...localSettings, filename_template: e.target.value })}
                className="w-full px-2 py-1 text-sm border border-gray-300 rounded-md"
              />
            </div>

            <div>
              <label className="block text-xs font-medium mb-0.5">FFmpeg Path</label>
              <div className="flex gap-1.5">
                <input
                  type="text"
                  value={localSettings.ffmpeg_path}
                  onChange={(e) => setLocalSettings({ ...localSettings, ffmpeg_path: e.target.value })}
                  placeholder="Using system PATH"
                  className="flex-1 px-2 py-1 text-sm border border-gray-300 rounded-md placeholder:text-gray-400 placeholder:italic"
                />
                <button
                  onClick={() => handleBrowse('ffmpeg_path')}
                  className="px-2 py-1 text-xs bg-gray-600 text-white rounded-md hover:bg-gray-700"
                >
                  Browse
                </button>
              </div>
            </div>

            <div>
              <label className="block text-xs font-medium mb-0.5">WhisperX Path</label>
              <div className="flex gap-1.5">
                <input
                  type="text"
                  value={localSettings.whisperx_path}
                  onChange={(e) => setLocalSettings({ ...localSettings, whisperx_path: e.target.value })}
                  placeholder="Using system PATH"
                  className="flex-1 px-2 py-1 text-sm border border-gray-300 rounded-md placeholder:text-gray-400 placeholder:italic"
                />
                <button
                  onClick={() => handleBrowse('whisperx_path')}
                  className="px-2 py-1 text-xs bg-gray-600 text-white rounded-md hover:bg-gray-700"
                >
                  Browse
                </button>
              </div>
            </div>

            <div>
              <label className="block text-xs font-medium mb-0.5">Browser (for YouTube scraping)</label>
              <select
                value={localSettings.browser}
                onChange={(e) => setLocalSettings({ ...localSettings, browser: e.target.value })}
                className="w-full px-2 py-1 text-sm border border-gray-300 rounded-md"
              >
                <option value="auto">Auto (Chrome, then Firefox)</option>
                <option value="chrome">Chrome</option>
                <option value="firefox">Firefox</option>
              </select>
            </div>

            <div>
              <label className="block text-xs font-medium mb-0.5">Browser Path (optional)</label>
              <div className="flex gap-1.5">
                <input
                  type="text"
                  value={localSettings.browser_path}
                  onChange={(e) => setLocalSettings({ ...localSettings, browser_path: e.target.value })}
                  placeholder="Auto-detected"
                  className="flex-1 px-2 py-1 text-sm border border-gray-300 rounded-md placeholder:text-gray-400 placeholder:italic"
                />
                <button
                  onClick={() => handleBrowse('browser_path')}
                  className="px-2 py-1 text-xs bg-gray-600 text-white rounded-md hover:bg-gray-700"
                >
                  Browse
                </button>
              </div>
            </div>
          </div>

          <div className="space-y-2">
            <div>
              <label className="block text-xs font-medium mb-0.5">Default Whisper Model</label>
              <select
                value={localSettings.default_model}
                onChange={(e) => setLocalSettings({ ...localSettings, default_model: e.target.value })}
                className="w-full px-2 py-1 text-sm border border-gray-300 rounded-md"
              >
                <option value="">Auto</option>
                <option value="base">Base</option>
                <option value="small">Small</option>
                <option value="medium">Medium</option>
                <option value="large-v2">Large-v2</option>
                <option value="large-v3">Large-v3</option>
                <option value="large-v3-turbo">Large-v3-turbo</option>
              </select>
            </div>

            <div>
              <label className="block text-xs font-medium mb-0.5">Default Transcript Source</label>
              <select
                value={localSettings.default_transcript_source}
                onChange={(e) => setLocalSettings({ ...localSettings, default_transcript_source: e.target.value as 'captions' | 'whisper' | 'both' })}
                className="w-full px-2 py-1 text-sm border border-gray-300 rounded-md"
              >
                <option value="captions">Captions (faster)</option>
                <option value="whisper">Whisper (more accurate)</option>
                <option value="both">Both (whisper + captions, no auto-clean)</option>
              </select>
            </div>

            <div>
              <label className="block text-xs font-medium mb-0.5">Claude Model Effort</label>
              <select
                value={localSettings.claude_model_effort}
                onChange={(e) =>
                  setLocalSettings({
                    ...localSettings,
                    claude_model_effort: e.target.value as Settings['claude_model_effort'],
                  })
                }
                className="w-full px-2 py-1 text-sm border border-gray-300 rounded-md"
              >
                <option value="low">Low</option>
                <option value="medium">Medium</option>
                <option value="high">High</option>
                <option value="xhigh">XHigh</option>
                <option value="max">Max</option>
              </select>
            </div>

            <div>
              <label className="block text-xs font-medium mb-0.5">Summarize Backend</label>
              <select
                value={localSettings.summarize_model}
                onChange={(e) =>
                  setLocalSettings({
                    ...localSettings,
                    summarize_model: e.target.value as Settings['summarize_model'],
                  })
                }
                className="w-full px-2 py-1 text-sm border border-gray-300 rounded-md"
              >
                <option value="claude">Claude</option>
                {ollamaAvailable === true && (
                  <option value="ollama">Ollama</option>
                )}
                {ollamaAvailable === false && localSettings.summarize_model === 'ollama' && (
                  <option value="ollama">Ollama (not found)</option>
                )}
              </select>
            </div>

            <div>
              <label className="block text-xs font-medium mb-0.5">Default Summarize Mode</label>
              <select
                value={localSettings.default_summarize_mode}
                onChange={(e) =>
                  setLocalSettings({
                    ...localSettings,
                    default_summarize_mode: e.target.value as Settings['default_summarize_mode'],
                  })
                }
                className="w-full px-2 py-1 text-sm border border-gray-300 rounded-md"
              >
                <option value="fast">Fast (skip cleaning; captions for URLs)</option>
                <option value="slow">Slow (full pipeline + cleaning)</option>
              </select>
            </div>

            <div className="space-y-1.5">
              <div>
                <label className="block text-xs font-medium mb-0.5">Automated Transcript Cleaning</label>
                <select
                  value={localSettings.auto_clean_transcript}
                  onChange={(e) =>
                    setLocalSettings({
                      ...localSettings,
                      auto_clean_transcript: e.target.value,
                    })
                  }
                  className="w-full px-2 py-1 text-sm border border-gray-300 rounded-md"
                >
                  <option value="off">Off</option>
                  <option value="claude">Claude</option>
                  {ollamaAvailable === null && (
                    <option value="ollama" disabled>Ollama (checking...)</option>
                  )}
                  {ollamaAvailable === true && (
                    <option value="ollama">Ollama</option>
                  )}
                  {ollamaAvailable === false && localSettings.auto_clean_transcript === 'ollama' && (
                    <option value="ollama">Ollama (not found)</option>
                  )}
                </select>
              </div>
            </div>

            <div className="space-y-1 pt-1">
              <label className="flex items-center gap-2">
                <input
                  type="checkbox"
                  checked={localSettings.default_video}
                  onChange={(e) => setLocalSettings({ ...localSettings, default_video: e.target.checked })}
                  className="w-3.5 h-3.5"
                />
                <span className="text-xs">Default: Video enabled</span>
              </label>

              <label className="flex items-center gap-2">
                <input
                  type="checkbox"
                  checked={localSettings.default_audio}
                  onChange={(e) => setLocalSettings({ ...localSettings, default_audio: e.target.checked })}
                  className="w-3.5 h-3.5"
                />
                <span className="text-xs">Default: Audio enabled</span>
              </label>

              <label className="flex items-center gap-2">
                <input
                  type="checkbox"
                  checked={localSettings.default_transcript}
                  onChange={(e) => {
                    const next = e.target.checked;
                    setLocalSettings({
                      ...localSettings,
                      default_transcript: next,
                      // Summarize requires transcript; keep them consistent.
                      default_summarize: next ? localSettings.default_summarize : false,
                    });
                  }}
                  className="w-3.5 h-3.5"
                />
                <span className="text-xs">Default: Transcript enabled</span>
              </label>

              <label
                className="flex items-center gap-2"
                style={{ opacity: localSettings.default_transcript ? 1 : 0.45 }}
              >
                <input
                  type="checkbox"
                  checked={localSettings.default_summarize}
                  disabled={!localSettings.default_transcript}
                  onChange={(e) => setLocalSettings({ ...localSettings, default_summarize: e.target.checked })}
                  className="w-3.5 h-3.5 disabled:cursor-not-allowed"
                />
                <span className="text-xs">Default: Summarize enabled</span>
              </label>

              <label className="flex items-center gap-2">
                <input
                  type="checkbox"
                  checked={localSettings.default_verbose}
                  onChange={(e) => setLocalSettings({ ...localSettings, default_verbose: e.target.checked })}
                  className="w-3.5 h-3.5"
                />
                <span className="text-xs">Default: Verbose logging</span>
              </label>

              <label className="flex items-center gap-2">
                <input
                  type="checkbox"
                  checked={localSettings.default_keep}
                  onChange={(e) => setLocalSettings({ ...localSettings, default_keep: e.target.checked })}
                  className="w-3.5 h-3.5"
                />
                <span className="text-xs">Default: Keep temporary files</span>
              </label>

              <label className="flex items-center gap-2">
                <input
                  type="checkbox"
                  checked={localSettings.default_clips_full_output}
                  onChange={(e) => setLocalSettings({ ...localSettings, default_clips_full_output: e.target.checked })}
                  className="w-3.5 h-3.5"
                />
                <span className="text-xs">Default: Output full media alongside clips</span>
              </label>
            </div>
          </div>
        </div>

        <div className="flex justify-end gap-2 mt-4">
          <button
            onClick={onClose}
            className="px-3 py-1.5 text-sm bg-gray-500 text-white rounded-md hover:brightness-110 active:scale-95 active:brightness-90 transition-transform duration-[80ms]"
          >
            Cancel
          </button>
          <button
            onClick={handleSave}
            className="px-3 py-1.5 text-sm bg-blue-600 text-white rounded-md hover:brightness-110 active:scale-95 active:brightness-90 transition-transform duration-[80ms]"
          >
            Save
          </button>
        </div>
      </div>
    </div>
  );
}
