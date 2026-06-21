use std::path::{Path, PathBuf};

use avtget_config::load_settings;
use avtget_domain::{BackendError, JobConfig, Result};

use crate::orchestration::local_media::{LocalMediaExecutionRequest, LocalMediaOrchestrator};
use crate::orchestration::non_local::{NonLocalExecutionRequest, NonLocalOrchestrator};
use crate::orchestration::routing::{decide_routing, RoutingDecision};

use super::channel_scrape_bridge::PythonChannelScrapeBridgeAdapter;
use super::ffmpeg::CliFfmpegAdapter;
use super::transcript_clean_bridge::PythonTranscriptCleanBridgeAdapter;
use super::video_metadata_bridge::PythonVideoMetadataBridgeAdapter;
use super::whisper_bridge::PythonWhisperBridgeAdapter;
use super::ytdlp::CliYtDlpAdapter;
use super::{AdapterOutcome, AdapterRequest, BackendAdapter};

#[derive(Debug, Default)]
pub struct OrchestratedBackendAdapter {
    ffmpeg: CliFfmpegAdapter,
    whisper: PythonWhisperBridgeAdapter,
    channel_scrape: PythonChannelScrapeBridgeAdapter,
    transcript_cleaner: PythonTranscriptCleanBridgeAdapter,
    video_metadata: PythonVideoMetadataBridgeAdapter,
}

impl BackendAdapter for OrchestratedBackendAdapter {
    fn run(&self, request: AdapterRequest) -> Result<AdapterOutcome> {
        let job_config: JobConfig = serde_json::from_str(&request.job_config_json)
            .map_err(|err| BackendError::InvalidJobConfig(err.to_string()))?;
        let settings = load_settings(&request.config_path)?;
        let ytdlp = CliYtDlpAdapter {
            cookies_browser: settings.browser.clone(),
        };

        match decide_routing(
            &job_config,
            &settings,
            request.python_whisper_bridge.is_some(),
        ) {
            RoutingDecision::LocalMedia { inputs } => {
                let bridge_script = request.python_whisper_bridge.as_ref().ok_or_else(|| {
                    BackendError::InvalidSettings(
                        "python whisper bridge path is required for Rust local-media route"
                            .to_owned(),
                    )
                })?;
                let cleaner_bridge = resolve_sibling_bridge(
                    request.python_whisper_bridge.as_deref(),
                    "transcript_clean_bridge.py",
                );
                let orchestrator = LocalMediaOrchestrator::new(
                    request.emitter.clone(),
                    request.cancel_token.clone(),
                    &self.ffmpeg,
                    &self.whisper,
                    &self.transcript_cleaner,
                );
                orchestrator.run(LocalMediaExecutionRequest {
                    config_path: &request.config_path,
                    job_config: &job_config,
                    settings: &settings,
                    plans: &inputs,
                    python_executable: &request.python_executable,
                    python_whisper_bridge: bridge_script,
                    cleaner_bridge: cleaner_bridge.as_deref(),
                })?;
                Ok(AdapterOutcome {
                    exit_code: Some(0),
                    terminal_event_seen: true,
                })
            }
            RoutingDecision::NonLocal { reason } => {
                let orchestrator = NonLocalOrchestrator::new(
                    &ytdlp,
                    &self.ffmpeg,
                    &self.whisper,
                    &self.channel_scrape,
                    &self.transcript_cleaner,
                    &self.video_metadata,
                );
                orchestrator.run(NonLocalExecutionRequest {
                    reason,
                    adapter_request: request,
                })
            }
        }
    }
}

/// Resolve a sibling Python bridge script relative to the whisper bridge path.
fn resolve_sibling_bridge(whisper_bridge: Option<&Path>, script_name: &str) -> Option<PathBuf> {
    let hint = whisper_bridge?;
    let dir = if hint.is_dir() {
        hint.to_path_buf()
    } else {
        hint.parent()?.to_path_buf()
    };
    let candidate = dir.join(script_name);
    if candidate.exists() {
        Some(candidate)
    } else {
        None
    }
}
