use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use avtget_domain::{AutoCleanTranscript, BackendError, Result, Settings};

pub fn resolve_config_path(program_dir: Option<&str>) -> PathBuf {
    if let Ok(path) = env::var("AVTGET_CONFIG_PATH") {
        if !path.trim().is_empty() {
            return PathBuf::from(path);
        }
    }

    if let Some(dir) = program_dir {
        let dir = dir.trim();
        if !dir.is_empty() {
            return PathBuf::from(dir).join("config.ini");
        }
    }

    PathBuf::from("config.ini")
}

pub fn load_settings(config_path: &Path) -> Result<Settings> {
    let content = fs::read_to_string(config_path).map_err(|err| {
        BackendError::InvalidSettings(format!(
            "failed to read config.ini at {}: {}",
            config_path.display(),
            err
        ))
    })?;

    let values = parse_default_section(&content);
    let base_dir = config_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let mut settings = Settings::with_base_paths(&base_dir);

    if let Some(value) = values.get("storage_directory") {
        settings.storage_directory = value.to_owned();
    }
    if let Some(value) = values.get("temp_directory") {
        settings.temp_directory = value.to_owned();
    }
    if let Some(value) = values.get("filename_template") {
        settings.filename_template = value.to_owned();
    }
    if let Some(value) = values.get("ffmpeg_path") {
        settings.ffmpeg_path = value.to_owned();
    }
    if let Some(value) = values.get("whisperx_path") {
        settings.whisperx_path = value.to_owned();
    }
    if let Some(value) = values.get("browser") {
        settings.browser = value.to_owned();
    }
    if let Some(value) = values.get("browser_path") {
        settings.browser_path = value.to_owned();
    }
    if let Some(value) = values.get("default_model") {
        settings.default_model = value.to_owned();
    }
    if let Some(value) = values.get("default_verbose") {
        settings.default_verbose = parse_bool(value, settings.default_verbose);
    }
    if let Some(value) = values.get("default_video") {
        settings.default_video = parse_bool(value, settings.default_video);
    }
    if let Some(value) = values.get("default_audio") {
        settings.default_audio = parse_bool(value, settings.default_audio);
    }
    if let Some(value) = values.get("default_transcript") {
        settings.default_transcript = parse_bool(value, settings.default_transcript);
    }
    if let Some(value) = values.get("default_keep") {
        settings.default_keep = parse_bool(value, settings.default_keep);
    }
    if let Some(value) = values.get("default_clips_full_output") {
        settings.default_clips_full_output = parse_bool(value, settings.default_clips_full_output);
    }
    if let Some(value) = values.get("default_transcript_source") {
        let normalized = value.trim().to_ascii_lowercase();
        settings.default_transcript_source = match normalized.as_str() {
            "whisper" => "whisper".to_owned(),
            "both" => "both".to_owned(),
            _ => "captions".to_owned(),
        };
    }
    if let Some(value) = values.get("auto_clean_transcript") {
        settings.auto_clean_transcript = AutoCleanTranscript::parse(value).selector();
    }
    if let Some(value) = values.get("summarize_model") {
        settings.summarize_model = parse_summarize_backend(value, &settings.summarize_model);
    }
    // Canonical key is `claude_model_effort`; fall back to v9 `summarize_effort`
    // so existing configs preserve the user's choice on upgrade.
    if let Some(value) = values
        .get("claude_model_effort")
        .or_else(|| values.get("summarize_effort"))
    {
        settings.claude_model_effort =
            parse_claude_model_effort(value, &settings.claude_model_effort);
    }

    Ok(settings)
}

/// Backend selector for the summarize step. Pre-v10 configs may carry specific
/// Claude model identifiers in this field; those migrate to "claude". The
/// Claude model itself is not configurable — CLI calls omit --model so the
/// user's saved Claude Code default applies.
fn parse_summarize_backend(raw: &str, default_value: &str) -> String {
    let lowered = raw.trim().to_ascii_lowercase();
    if lowered.is_empty() {
        return default_value.to_owned();
    }
    if lowered == "ollama" {
        return "ollama".to_owned();
    }
    if lowered == "claude" || lowered.starts_with("claude-") || lowered == "anthropic" {
        return "claude".to_owned();
    }
    default_value.to_owned()
}

fn parse_claude_model_effort(raw: &str, default_value: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "low" => "low".to_owned(),
        "medium" => "medium".to_owned(),
        "high" => "high".to_owned(),
        "xhigh" => "xhigh".to_owned(),
        "max" => "max".to_owned(),
        _ => default_value.to_owned(),
    }
}

pub fn effective_keep(settings: &Settings, job_keep: Option<bool>) -> bool {
    job_keep.unwrap_or(settings.default_keep)
}

pub fn resolve_relative_to_config(config_path: &Path, configured_path: &str) -> PathBuf {
    let candidate = PathBuf::from(configured_path);
    if candidate.is_absolute() {
        return candidate;
    }

    let base = config_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join(candidate)
}

fn parse_default_section(raw: &str) -> HashMap<String, String> {
    let mut values = HashMap::new();
    let mut in_default = true;

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with(';') || trimmed.starts_with('#') {
            continue;
        }

        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            let section = &trimmed[1..trimmed.len() - 1];
            in_default = section.eq_ignore_ascii_case("default");
            continue;
        }

        if !in_default {
            continue;
        }

        if let Some((key, value)) = trimmed.split_once('=') {
            values.insert(key.trim().to_ascii_lowercase(), value.trim().to_owned());
        }
    }

    values
}

fn parse_bool(raw: &str, default: bool) -> bool {
    let lowered = raw.trim().to_ascii_lowercase();
    if lowered.is_empty() {
        return default;
    }

    match lowered.as_str() {
        "1" | "true" | "yes" | "on" => true,
        "0" | "false" | "no" | "off" => false,
        _ => default,
    }
}
