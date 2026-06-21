use std::path::PathBuf;

use avtget_domain::{ClipRange, JobConfig, Settings};

const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "wav", "m4a", "flac", "aac", "ogg", "opus", "wma", "aiff", "alac",
];
const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mkv", "mov", "avi", "m4v", "webm", "wmv", "flv", "ts", "m2ts", "mpeg", "mpg", "3gp",
    "vob",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalInputKind {
    Audio,
    Video,
}

#[derive(Debug, Clone)]
pub struct LocalInputPlan {
    pub input_id: String,
    pub path: PathBuf,
    pub kind: LocalInputKind,
    pub clips: Vec<ClipRange>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NonLocalReason {
    RefreshIndex,
    EmptyInputResume,
    ManualOrSkipSelection,
    MissingWhisperBridge,
    UrlPodcastChannelInput,
    TranscriptFileInput,
    UnsupportedLocalExtension { extension: String },
    NoEffectiveModes,
    EmptyInputEntry,
    NoLocalInputsResolved,
}

#[derive(Debug, Clone)]
pub enum RoutingDecision {
    LocalMedia { inputs: Vec<LocalInputPlan> },
    NonLocal { reason: NonLocalReason },
}

pub fn decide_routing(
    job_config: &JobConfig,
    settings: &Settings,
    whisper_bridge_available: bool,
) -> RoutingDecision {
    if job_config.refresh_index.unwrap_or(false) {
        return RoutingDecision::NonLocal {
            reason: NonLocalReason::RefreshIndex,
        };
    }
    if job_config.inputs.is_empty() {
        return RoutingDecision::NonLocal {
            reason: NonLocalReason::EmptyInputResume,
        };
    }
    if job_config
        .manual_clean_inputs
        .as_ref()
        .map(|value| !value.is_empty())
        .unwrap_or(false)
        || job_config
            .skip_inputs
            .as_ref()
            .map(|value| !value.is_empty())
            .unwrap_or(false)
    {
        return RoutingDecision::NonLocal {
            reason: NonLocalReason::ManualOrSkipSelection,
        };
    }

    let (video_enabled, audio_enabled, transcript_enabled) = effective_modes(job_config, settings);
    if transcript_enabled && !whisper_bridge_available {
        return RoutingDecision::NonLocal {
            reason: NonLocalReason::MissingWhisperBridge,
        };
    }
    if !(video_enabled || audio_enabled || transcript_enabled) {
        return RoutingDecision::NonLocal {
            reason: NonLocalReason::NoEffectiveModes,
        };
    }

    let mut plans = Vec::with_capacity(job_config.inputs.len());
    for (index, raw_input) in job_config.inputs.iter().enumerate() {
        let input = raw_input.trim();
        if input.is_empty() {
            return RoutingDecision::NonLocal {
                reason: NonLocalReason::EmptyInputEntry,
            };
        }
        if looks_like_url(input) {
            return RoutingDecision::NonLocal {
                reason: NonLocalReason::UrlPodcastChannelInput,
            };
        }

        let path = PathBuf::from(input);
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| value.to_ascii_lowercase())
            .unwrap_or_default();
        if extension == "txt" {
            return RoutingDecision::NonLocal {
                reason: NonLocalReason::TranscriptFileInput,
            };
        }

        let kind = if AUDIO_EXTENSIONS.contains(&extension.as_str()) {
            LocalInputKind::Audio
        } else if VIDEO_EXTENSIONS.contains(&extension.as_str()) {
            LocalInputKind::Video
        } else {
            return RoutingDecision::NonLocal {
                reason: NonLocalReason::UnsupportedLocalExtension { extension },
            };
        };

        plans.push(LocalInputPlan {
            input_id: raw_input.clone(),
            path,
            kind,
            clips: clips_for_index(job_config, index),
        });
    }

    if plans.is_empty() {
        return RoutingDecision::NonLocal {
            reason: NonLocalReason::NoLocalInputsResolved,
        };
    }

    RoutingDecision::LocalMedia { inputs: plans }
}

pub fn effective_modes(job_config: &JobConfig, settings: &Settings) -> (bool, bool, bool) {
    if !(job_config.video || job_config.audio || job_config.transcript) {
        return (
            settings.default_video,
            settings.default_audio,
            settings.default_transcript,
        );
    }
    (job_config.video, job_config.audio, job_config.transcript)
}

fn clips_for_index(job_config: &JobConfig, index: usize) -> Vec<ClipRange> {
    job_config
        .clip_timestamps
        .as_ref()
        .and_then(|all_clips| all_clips.get(index).cloned())
        .unwrap_or_default()
}

fn looks_like_url(value: &str) -> bool {
    let lowered = value.to_ascii_lowercase();
    lowered.starts_with("http://") || lowered.starts_with("https://") || lowered.starts_with("www.")
}
