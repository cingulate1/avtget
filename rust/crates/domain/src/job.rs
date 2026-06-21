use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipRange {
    pub start: String,
    pub end: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobConfig {
    pub inputs: Vec<String>,
    pub video: bool,
    pub audio: bool,
    pub transcript: bool,
    #[serde(default)]
    pub gpu: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub cq: Option<i64>,
    #[serde(default)]
    pub preset: Option<String>,
    #[serde(default)]
    pub keep: Option<bool>,
    #[serde(default)]
    pub refresh_index: Option<bool>,
    #[serde(default)]
    pub verbose: Option<bool>,
    #[serde(default)]
    pub program_dir: Option<String>,
    #[serde(default)]
    pub timeframe: Option<String>,
    #[serde(default)]
    pub transcript_source: Option<String>,
    // Per-job override for the transcript-cleaning selector. When Some, takes
    // precedence over Settings::auto_clean_transcript. Frontend sets this to
    // "off" when submit mode is Summarize+Fast so the cleaning step is bypassed
    // for that batch without the user having to flip the global setting.
    #[serde(default)]
    pub auto_clean_transcript: Option<String>,
    #[serde(default, rename = "clipTimestamps")]
    pub clip_timestamps: Option<Vec<Vec<ClipRange>>>,
    #[serde(default, rename = "episodeLimit")]
    pub episode_limit: Option<i64>,
    #[serde(default, rename = "manualCleanInputs")]
    pub manual_clean_inputs: Option<Vec<String>>,
    #[serde(default, rename = "skipInputs")]
    pub skip_inputs: Option<Vec<String>>,
    // When true, the backend runs a post-transcript summarization pass on each
    // transcript before moving to the next item. Drives sequential
    // clean → summarize → next, replacing the old frontend-driven chain.
    #[serde(default)]
    pub summarize: bool,
}

impl JobConfig {
    pub fn effective_keep(&self, default_keep: bool) -> bool {
        self.keep.unwrap_or(default_keep)
    }
}

#[derive(Debug, Clone)]
pub struct Settings {
    pub storage_directory: String,
    pub temp_directory: String,
    pub filename_template: String,
    pub ffmpeg_path: String,
    pub whisperx_path: String,
    pub browser: String,
    pub browser_path: String,
    pub default_model: String,
    pub default_verbose: bool,
    pub default_video: bool,
    pub default_audio: bool,
    pub default_transcript: bool,
    pub default_keep: bool,
    pub default_clips_full_output: bool,
    pub default_transcript_source: String,
    pub auto_clean_transcript: String,
    // Summarize step settings, read by the backend orchestrator so the
    // clean → summarize chain runs entirely on the backend side. The Claude
    // model is not configurable — CLI calls omit --model so the user's saved
    // Claude Code default applies.
    pub summarize_model: String,
    pub claude_model_effort: String,
}

impl Settings {
    pub fn with_base_paths(base_dir: &Path) -> Self {
        Self {
            storage_directory: base_dir.join("storage").to_string_lossy().into_owned(),
            temp_directory: base_dir.join("avtget_temp").to_string_lossy().into_owned(),
            filename_template: "%channelname - %videotitle".to_owned(),
            ffmpeg_path: String::new(),
            whisperx_path: String::new(),
            browser: "auto".to_owned(),
            browser_path: String::new(),
            default_model: String::new(),
            default_verbose: false,
            default_video: true,
            default_audio: true,
            default_transcript: false,
            default_keep: false,
            default_clips_full_output: true,
            default_transcript_source: "captions".to_owned(),
            auto_clean_transcript: "off".to_owned(),
            summarize_model: "claude".to_owned(),
            claude_model_effort: "medium".to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutoCleanTranscript {
    Off,
    Claude,
    Ollama,
}

impl AutoCleanTranscript {
    pub fn parse(raw: &str) -> Self {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Self::Off;
        }

        match trimmed.to_ascii_lowercase().as_str() {
            "off" => Self::Off,
            "claude" => Self::Claude,
            // "ollama" or any unrecognized value (old model names, retired
            // "gemini") → Ollama
            _ => Self::Ollama,
        }
    }

    pub fn selector(&self) -> String {
        match self {
            Self::Off => "off".to_owned(),
            Self::Claude => "claude".to_owned(),
            Self::Ollama => "ollama".to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AutoCleanTranscript;

    #[test]
    fn auto_clean_selector_parses_explicit_values() {
        assert_eq!(AutoCleanTranscript::parse("off"), AutoCleanTranscript::Off);
        assert_eq!(
            AutoCleanTranscript::parse("claude"),
            AutoCleanTranscript::Claude
        );
        assert_eq!(
            AutoCleanTranscript::parse("ollama"),
            AutoCleanTranscript::Ollama
        );
        // Retired "gemini" selector should map to Ollama for backward compat
        assert_eq!(
            AutoCleanTranscript::parse("gemini"),
            AutoCleanTranscript::Ollama
        );
        // Old model names should map to Ollama for backward compat
        assert_eq!(
            AutoCleanTranscript::parse("qwen3:32b"),
            AutoCleanTranscript::Ollama
        );
    }
}
