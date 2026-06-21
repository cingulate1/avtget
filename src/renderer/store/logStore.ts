import { create } from 'zustand';

// Patterns to suppress from verbose log output (string-level filtering only)
const SUPPRESSED_PATTERNS = [
  // yt-dlp SABR streaming warnings
  'Some web_safari client https formats have been skipped as they are missing a url. YouTube is forcing SABR streaming',
  'Some web client https formats have been skipped as they are missing a url. YouTube is forcing SABR streaming',
  // torchaudio deprecation warnings
  'torchaudio._backend.list_audio_backends has been deprecated',
  // huggingface hf_xet warning
  'Xet Storage is enabled for this repo, but the \'hf_xet\' package is not installed',
  // huggingface symlinks warning
  'huggingface_hub` cache-system uses symlinks by default to efficiently store duplicated files but your machine does not support them',
  // pyannote version mismatch warnings
  'Model was trained with pyannote.audio 0.0.1, yours is',
  'Model was trained with torch 1.10.0+cu102, yours is',
  // pyannote TF32 reproducibility warning
  'TensorFloat-32 (TF32) has been disabled as it might lead to reproducibility issues',
];

// Check if a message should be suppressed
function shouldSuppressMessage(message: string): boolean {
  return SUPPRESSED_PATTERNS.some(pattern => message.includes(pattern));
}

interface LogEntry {
  message: string;
  timestamp: number;
  isError: boolean;
}

interface LogState {
  logs: LogEntry[];
  statusLine: string;
  hasJobError: boolean;  // True only when an actual job_error event is received

  addLog: (message: string, isError?: boolean) => void;
  setStatusLine: (message: string) => void;
  setHasJobError: (hasError: boolean) => void;
  clearLogs: () => void;
}

export const useLogStore = create<LogState>((set) => ({
  logs: [],
  statusLine: '',
  hasJobError: false,

  addLog: (message, isError = false) => set((state) => {
    // Filter out suppressed messages at frontend level
    if (shouldSuppressMessage(message)) {
      return state; // Return unchanged state, effectively skipping this message
    }
    return {
      logs: [...state.logs, { message, timestamp: Date.now(), isError }]
    };
  }),

  setStatusLine: (message) => set({ statusLine: message }),

  setHasJobError: (hasError) => set({ hasJobError: hasError }),

  clearLogs: () => set({ logs: [], statusLine: '', hasJobError: false }),
}));
