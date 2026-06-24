pub mod ffmpeg;
pub mod orchestrated_backend;
pub mod temp_storage;
pub mod transcript_clean_bridge;
pub mod video_metadata_bridge;
pub mod whisper_bridge;
pub mod ytdlp;

use std::path::{Path, PathBuf};

use avtget_domain::{ClipRange, Result};

use crate::cancel::CancellationToken;
use crate::events::EventEmitter;

pub struct AdapterRequest {
    pub config_path: PathBuf,
    pub job_config_json: String,
    pub python_executable: String,
    pub python_whisper_bridge: Option<PathBuf>,
    pub emitter: EventEmitter,
    pub cancel_token: CancellationToken,
}

pub struct AdapterOutcome {
    pub exit_code: Option<i32>,
    pub terminal_event_seen: bool,
}

pub trait BackendAdapter: Send + Sync {
    fn run(&self, request: AdapterRequest) -> Result<AdapterOutcome>;
}

pub trait TempStoreAdapter: Send + Sync {
    fn prepare_temp_directory(&self, directory: &Path, keep_files: bool) -> Result<()>;
}

pub trait YtDlpAdapter: Send + Sync {
    fn download_media(
        &self,
        python_executable: &str,
        url: &str,
        format_selector: &str,
        output_template: &str,
        ffmpeg_path: Option<&str>,
        verbose: bool,
    ) -> Result<()>;

    fn download_subtitles(
        &self,
        python_executable: &str,
        url: &str,
        output_template: &str,
        ffmpeg_path: Option<&str>,
        verbose: bool,
    ) -> Result<()>;
}

pub trait FfmpegAdapter: Send + Sync {
    fn trim_media(
        &self,
        input_path: &Path,
        output_path: &Path,
        start_seconds: f64,
        end_seconds: f64,
        ffmpeg_path: Option<&str>,
        verbose: bool,
    ) -> Result<bool>;

    fn video_to_audio(
        &self,
        video_path: &Path,
        audio_path: &Path,
        ffmpeg_path: Option<&str>,
        verbose: bool,
    ) -> Result<bool>;

    fn audio_to_mp3(
        &self,
        audio_path: &Path,
        output_path: &Path,
        ffmpeg_path: Option<&str>,
        verbose: bool,
    ) -> Result<bool>;

    /// Convert any audio or video to a 16 KHz mono WAV suitable for WhisperX.
    fn to_whisper_wav(
        &self,
        input_path: &Path,
        output_path: &Path,
        ffmpeg_path: Option<&str>,
        verbose: bool,
    ) -> Result<bool>;
}

pub struct WhisperBridgeRequest {
    pub python_executable: String,
    pub bridge_script: PathBuf,
    pub audio_path: PathBuf,
    pub output_dir: PathBuf,
    pub temp_dir: PathBuf,
    pub output_filestem: String,
    pub model: String,
    pub gpu: String,
    pub clips: Vec<ClipRange>,
    pub clips_full_output: bool,
    pub whisperx_path: Option<String>,
    pub ffmpeg_path: Option<String>,
}

pub trait WhisperBridgeAdapter: Send + Sync {
    fn transcribe(&self, request: WhisperBridgeRequest) -> Result<()>;
}

pub struct ChannelScrapeRequest {
    pub python_executable: String,
    pub channel_url: String,
    pub timeframe_days: i64,
    pub verbose: bool,
}

pub trait ChannelScrapeAdapter: Send + Sync {
    fn scrape_channel_urls(&self, request: ChannelScrapeRequest) -> Result<Vec<String>>;
}

#[derive(Debug, Clone)]
pub struct VideoMetadata {
    pub title: Option<String>,
    pub channel: Option<String>,
}

pub struct VideoMetadataRequest {
    pub python_executable: String,
    pub bridge_script: PathBuf,
    pub url: String,
    pub browser: String,
    pub browser_path: Option<String>,
    pub verbose: bool,
}

pub trait VideoMetadataAdapter: Send + Sync {
    fn fetch_metadata(&self, request: VideoMetadataRequest) -> Result<VideoMetadata>;
}

pub struct TranscriptCleanRequest {
    pub python_executable: String,
    pub bridge_script: PathBuf,
    pub transcript_path: PathBuf,
    pub output_path: PathBuf,
    // "claude" never routes here — Claude cleaning runs in-process via
    // postprocess.rs. Only "ollama" reaches this adapter.
    pub cleaner: String,
    /// Path to the debug log file so the bridge can mirror Python stdout there.
    pub log_file_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Default)]
pub struct TranscriptCleanOutcome {
    pub cleaner: Option<String>,
    pub provider: Option<String>,
    pub used_sharding: Option<bool>,
    pub shards_total: Option<usize>,
    pub raw_chars: Option<usize>,
    pub cleaned_chars: Option<usize>,
}

pub trait TranscriptCleanerAdapter: Send + Sync {
    fn clean_transcript(&self, request: TranscriptCleanRequest) -> Result<TranscriptCleanOutcome>;
}
