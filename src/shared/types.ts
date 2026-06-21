export interface ClipRange {
  start: string;
  end: string;
}

export interface JobConfig {
  inputs: string[];
  video: boolean;
  audio: boolean;
  transcript: boolean;
  gpu?: string;
  model?: string;
  cq?: number;
  preset?: string;
  keep?: boolean;
  refresh_index?: boolean;
  verbose?: boolean;
  program_dir?: string;
  timeframe?: string;
  transcript_source?: string;
  // Per-job override for cleaning. When set, takes precedence over
  // settings.auto_clean_transcript. Used by the Summarize+Fast submit path to
  // force cleaning off for the batch without mutating the global setting.
  auto_clean_transcript?: string;
  clipTimestamps?: ClipRange[][];  // Array of clip ranges per input URL
  episodeLimit?: number;  // Max episodes to process for podcasts (undefined = all)
  // Inputs that should be cleaned via Ollama even if auto_clean is off (user clicked Yes)
  manualCleanInputs?: string[];
  // Inputs that should be skipped entirely (user clicked No)
  skipInputs?: string[];
  // When true the backend runs a post-transcript summarize pass on each
  // item before moving on. The backend (not the frontend) owns the
  // clean → summarize chain, so this flag is the only thing the UI needs
  // to send when the user submits with the Summarize checkbox on.
  summarize?: boolean;
  // Absolute path to a frozen config.ini snapshot taken at GO time. The Tauri
  // shell points the spawned backend's AVTGET_CONFIG_PATH at this file (and
  // strips the field from the payload), locking in the config-only settings
  // (summarize_model, claude_model_effort, filename_template, paths, …) so a
  // queued/running job is immune to later edits.
  config_snapshot_path?: string;
}

export interface Settings {
  storage_directory: string;
  temp_directory: string;
  filename_template: string;
  ffmpeg_path: string;
  whisperx_path: string;
  browser: string;
  browser_path: string;
  default_model: string;
  default_verbose: boolean;
  default_video: boolean;
  default_audio: boolean;
  default_transcript: boolean;
  default_keep: boolean;
  default_clips_full_output: boolean;
  default_transcript_source: 'captions' | 'whisper' | 'both';
  auto_clean_transcript: string;  // 'off', 'claude' (Claude Code), 'ollama'
  http_server_enabled: boolean;
  http_server_port: number;
  http_server_token: string;
  default_summarize: boolean;
  default_summarize_mode: SummarizeMode;
  summarize_model: SummarizeBackend;
  claude_model_effort: ClaudeModelEffort;
}

// Backend choice for the summarize step. Claude routes to Claude Code subprocess.
export type SummarizeBackend = 'claude' | 'ollama';

// Fast = yt-captions only, single-turn Claude Code summary.
// Slow = honors V/A/T config, runs clean-transcript skill via two-turn Claude Code, then summarizes.
export type SummarizeMode = 'fast' | 'slow';

// Effort level for Claude CLI calls. The model itself is not configurable:
// `claude` is invoked without --model, so it resolves to the user's saved
// Claude Code default. Levels the model doesn't support fall back gracefully.
export type ClaudeModelEffort = 'low' | 'medium' | 'high' | 'xhigh' | 'max';

export type ExternalJobPreset = 'archive_video' | 'save_audio' | 'save_transcript' | 'summarize';

export interface ExternalJobRequest {
  url: string;
  preset: ExternalJobPreset;
  modes: {
    video: boolean;
    audio: boolean;
    transcript: boolean;
    summarize: boolean;
  };
  overrides?: {
    transcript_source?: 'captions' | 'whisper' | 'both';
    auto_clean_transcript?: string;
  };
}

export type JobStatus = 'queued' | 'running' | 'completed' | 'failed' | 'cancelled';
export type ArtifactStatus = JobStatus | 'not_requested' | 'skipped' | 'warning';

export interface JobItem {
  itemId: string;
  displayName: string;
  status: JobStatus;
  artifacts: {
    video: ArtifactStatus;
    audio: ArtifactStatus;
    transcript: ArtifactStatus;
    summary: ArtifactStatus;
  };
}

export interface BackendEvent {
  type: 'log' | 'progress' | 'status_change' | 'stage_count' | 'artifact_status' | 'job_finished' | 'job_error' | 'backend_exited';
  [key: string]: any;
}

export interface LogEvent extends BackendEvent {
  type: 'log';
  message: string;
}

export interface ProgressEvent extends BackendEvent {
  type: 'progress';
  percent: number;
  stage: string;
}

export interface StatusChangeEvent extends BackendEvent {
  type: 'status_change';
  item_id: string;
  status: JobStatus;
}

export interface StageCountEvent extends BackendEvent {
  type: 'stage_count';
  stage_name: string;
  current: number;
  total: number;
}

export interface ArtifactStatusEvent extends BackendEvent {
  type: 'artifact_status';
  item_id: string;
  artifact: 'video' | 'audio' | 'transcript' | 'summary' | '__filestem__';
  status: string;
}

export interface JobFinishedEvent extends BackendEvent {
  type: 'job_finished';
  summary: string;
}

export interface JobErrorEvent extends BackendEvent {
  type: 'job_error';
  error: string;
}
