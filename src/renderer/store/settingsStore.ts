import { create } from 'zustand';
import type { Settings } from '@/shared/types';
import { getDesktopAPI } from '../desktopApi';

const DEFAULT_SETTINGS: Settings = {
  storage_directory: '',
  temp_directory: '',
  filename_template: '%channelname - %videotitle',
  ffmpeg_path: '',
  whisperx_path: '',
  browser: 'auto',
  browser_path: '',
  default_model: '',
  default_verbose: false,
  default_video: true,
  default_audio: true,
  default_transcript: false,
  default_keep: false,
  default_clips_full_output: true,
  default_transcript_source: 'captions',
  auto_clean_transcript: 'off',
  http_server_enabled: true,
  http_server_port: 47923,
  http_server_token: '',
  default_summarize: false,
  default_summarize_mode: 'fast',
  summarize_model: 'claude',
  claude_model_effort: 'medium',
};

interface SettingsState {
  settings: Settings;
  isLoaded: boolean;
  loadSettings: () => Promise<void>;
  saveSettings: (settings: Settings) => Promise<void>;
  updateSetting: <K extends keyof Settings>(key: K, value: Settings[K]) => void;
}

export const useSettingsStore = create<SettingsState>((set, get) => ({
  settings: DEFAULT_SETTINGS,
  isLoaded: false,

  loadSettings: async () => {
    const settings = await getDesktopAPI().getConfig();
    set({ settings, isLoaded: true });
  },

  saveSettings: async (settings) => {
    await getDesktopAPI().saveConfig(settings);
    set({ settings });
  },

  updateSetting: (key, value) => {
    const { settings } = get();
    set({ settings: { ...settings, [key]: value } });
  },
}));
