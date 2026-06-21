#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Default)]
struct BackendState {
    process: Arc<Mutex<Option<ManagedProcess>>>,
    // Opened once in setup (after temp cleanup). Truncating the file on each
    // start_job would wipe prior jobs in the same session.
    log_file: LogFile,
}

struct ManagedProcess {
    child: Child,
    // Per-job frozen config snapshot (set when the job was dispatched with a
    // `config_snapshot_path`). Removed when the process exits so avtget_temp
    // doesn't accumulate snapshots over a session.
    config_snapshot: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SettingsPayload {
    storage_directory: String,
    temp_directory: String,
    filename_template: String,
    ffmpeg_path: String,
    whisperx_path: String,
    browser: String,
    browser_path: String,
    default_model: String,
    default_verbose: bool,
    default_video: bool,
    default_audio: bool,
    default_transcript: bool,
    default_keep: bool,
    default_clips_full_output: bool,
    default_transcript_source: String,
    auto_clean_transcript: String,
    http_server_enabled: bool,
    http_server_port: u16,
    http_server_token: String,
    default_summarize: bool,
    default_summarize_mode: String,
    // Backend selector for the summarize step: "claude" or "ollama".
    summarize_model: String,
    // Effort level passed via `claude --effort`. Applies to both clean and
    // summarize Claude paths. The model is never passed explicitly — `claude`
    // resolves it from the user's saved Claude Code default, and effort levels
    // the resolved model doesn't support fall back to the nearest one it does.
    claude_model_effort: String,
}

// Live working-state mirror of the five main-window checkboxes. Written to
// config.ini immediately on every toggle (unidirectional GUI -> file) and
// deliberately kept distinct from the `default_*` keys above: the defaults are
// the Save-gated seed the GUI reads once on startup, while these reflect the
// user's current selection. Nothing reads them back into the GUI.
#[derive(Debug, Clone, Copy, Deserialize)]
struct LiveModes {
    video: bool,
    audio: bool,
    transcript: bool,
    summarize: bool,
    verbose: bool,
}

impl LiveModes {
    // Fallback when the file has no live keys yet (fresh config, or an upgrade
    // from a version that predates them): seed from the default_* settings.
    fn from_default_settings(settings: &SettingsPayload) -> Self {
        Self {
            video: settings.default_video,
            audio: settings.default_audio,
            transcript: settings.default_transcript,
            summarize: settings.default_summarize,
            verbose: settings.default_verbose,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct SaveDialogResponse {
    canceled: bool,
    #[serde(rename = "filePath")]
    file_path: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct ArtifactArgs {
    artifact: String,
    filestem: String,
}

fn repo_root_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn app_base_path() -> PathBuf {
    if let Ok(path) = env::var("AVTGET_BASE_DIR") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }

    // CARGO_MANIFEST_DIR is baked in at compile time and points to src-tauri/.
    // The version root is one level up — this works for both dev builds and
    // anyone who clones the repo and runs `npm run package`.
    repo_root_path()
}

fn version_root_path() -> PathBuf {
    let base = app_base_path();
    let is_runtime_dir = base
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.eq_ignore_ascii_case("runtime"))
        .unwrap_or(false);
    if is_runtime_dir {
        return base.parent().map(Path::to_path_buf).unwrap_or(base);
    }
    base
}

fn config_path() -> PathBuf {
    if let Ok(path) = env::var("AVTGET_CONFIG_PATH") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    version_root_path().join("config.ini")
}

fn default_settings(base_dir: &Path) -> SettingsPayload {
    SettingsPayload {
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
        http_server_enabled: true,
        http_server_port: 47923,
        http_server_token: String::new(),
        default_summarize: false,
        default_summarize_mode: "fast".to_owned(),
        summarize_model: "claude".to_owned(),
        claude_model_effort: "medium".to_owned(),
    }
}

fn generate_http_token() -> String {
    let mut bytes = [0u8; 16];
    if getrandom::getrandom(&mut bytes).is_err() {
        // Fallback: mix time + process id. Not cryptographically strong, but
        // this token only gates a local HTTP endpoint on a single-user machine,
        // and getrandom never fails on Windows in practice.
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let pid = std::process::id() as u128;
        let seed = nanos.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(pid);
        for (i, byte) in bytes.iter_mut().enumerate() {
            *byte = ((seed >> (i * 4)) & 0xFF) as u8;
        }
    }
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn parse_bool(value: Option<&String>, default: bool) -> bool {
    let Some(value) = value else {
        return default;
    };
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => true,
        "0" | "false" | "no" | "off" => false,
        _ => default,
    }
}

fn parse_transcript_clean(raw: Option<&String>, default_value: &str) -> String {
    let Some(raw) = raw else {
        return default_value.to_owned();
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return "off".to_owned();
    }
    let lowered = trimmed.to_ascii_lowercase();
    if ["0", "false", "no", "off", "none", "disabled"].contains(&lowered.as_str()) {
        return "off".to_owned();
    }
    if ["1", "true", "yes", "on"].contains(&lowered.as_str()) {
        return "claude".to_owned();
    }
    if lowered == "claude" || lowered == "anthropic" {
        return "claude".to_owned();
    }
    // "ollama" or any unrecognized value (old model names, retired "gemini") → ollama
    "ollama".to_owned()
}

fn parse_summarize_mode(raw: Option<&String>, default_value: &str) -> String {
    let Some(raw) = raw else {
        return default_value.to_owned();
    };
    match raw.trim().to_ascii_lowercase().as_str() {
        "fast" => "fast".to_owned(),
        "slow" => "slow".to_owned(),
        _ => default_value.to_owned(),
    }
}

// Backend selector for the summarize step. Pre-v10 configs may carry specific
// Claude model identifiers (claude-opus-4-6 / claude-sonnet-4-6 / claude-haiku-4-5)
// in this field; those are migrated to "claude". The Claude model itself is not
// configurable — the CLI resolves the user's saved Claude Code default.
fn parse_summarize_backend(raw: Option<&String>, default_value: &str) -> String {
    let Some(raw) = raw else {
        return default_value.to_owned();
    };
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

fn parse_claude_model_effort(raw: Option<&String>, default_value: &str) -> String {
    let Some(raw) = raw else {
        return default_value.to_owned();
    };
    match raw.trim().to_ascii_lowercase().as_str() {
        "low" => "low".to_owned(),
        "medium" => "medium".to_owned(),
        "high" => "high".to_owned(),
        "xhigh" => "xhigh".to_owned(),
        "max" => "max".to_owned(),
        _ => default_value.to_owned(),
    }
}

fn parse_ini_default_section(raw: &str) -> HashMap<String, String> {
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

// Serializes config.ini reads and writes so a GO-time snapshot can never
// observe a partially-rewritten file (e.g. an instant checkbox toggle landing
// at the same instant as a freeze). Each critical section wraps a SINGLE fs
// operation — never hold this across another config op, to stay deadlock-free.
fn config_guard() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

// Absolute temp directory. temp_directory is normalized to absolute on load
// (see load_or_create_config), so this is normally a direct passthrough; the
// relative branch is a defensive fallback mirroring the backend crate's own
// resolve_relative_to_config behavior.
fn resolve_temp_dir(settings: &SettingsPayload) -> PathBuf {
    let candidate = PathBuf::from(&settings.temp_directory);
    if candidate.is_absolute() {
        return candidate;
    }
    config_path()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(candidate)
}

// Unique id for snapshot filenames: wall-clock nanos times a process-local
// counter, so two freezes in the same nanosecond can't collide.
fn next_snapshot_id() -> u128 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed) as u128;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    nanos.wrapping_mul(1000).wrapping_add(counter % 1000)
}

fn load_or_create_config() -> Result<SettingsPayload, String> {
    let path = config_path();
    let base = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let defaults = default_settings(&base);

    if !path.exists() {
        save_config_file(&defaults)?;
        return Ok(defaults);
    }

    let content = {
        let _guard = config_guard();
        fs::read_to_string(&path)
    }
    .map_err(|err| format!("Failed to read config at {}: {err}", path.display()))?;
    let values = parse_ini_default_section(&content);

    let http_token = values
        .get("http_server_token")
        .cloned()
        .unwrap_or_default();
    let http_token_needs_generation = http_token.trim().is_empty();

    let http_port = values
        .get("http_server_port")
        .and_then(|raw| raw.trim().parse::<u16>().ok())
        .unwrap_or(defaults.http_server_port);

    let mut settings = SettingsPayload {
        storage_directory: values
            .get("storage_directory")
            .cloned()
            .unwrap_or_else(|| defaults.storage_directory.clone()),
        temp_directory: values
            .get("temp_directory")
            .cloned()
            .unwrap_or_else(|| defaults.temp_directory.clone()),
        filename_template: values
            .get("filename_template")
            .cloned()
            .unwrap_or_else(|| defaults.filename_template.clone()),
        ffmpeg_path: values
            .get("ffmpeg_path")
            .cloned()
            .unwrap_or_else(|| defaults.ffmpeg_path.clone()),
        whisperx_path: values
            .get("whisperx_path")
            .cloned()
            .unwrap_or_else(|| defaults.whisperx_path.clone()),
        browser: values
            .get("browser")
            .cloned()
            .unwrap_or_else(|| defaults.browser.clone()),
        browser_path: values
            .get("browser_path")
            .cloned()
            .unwrap_or_else(|| defaults.browser_path.clone()),
        default_model: values
            .get("default_model")
            .cloned()
            .unwrap_or_else(|| defaults.default_model.clone()),
        default_verbose: parse_bool(values.get("default_verbose"), defaults.default_verbose),
        default_video: parse_bool(values.get("default_video"), defaults.default_video),
        default_audio: parse_bool(values.get("default_audio"), defaults.default_audio),
        default_transcript: parse_bool(
            values.get("default_transcript"),
            defaults.default_transcript,
        ),
        default_keep: parse_bool(values.get("default_keep"), defaults.default_keep),
        default_clips_full_output: parse_bool(values.get("default_clips_full_output"), defaults.default_clips_full_output),
        default_transcript_source: values
            .get("default_transcript_source")
            .cloned()
            .unwrap_or_else(|| defaults.default_transcript_source.clone()),
        auto_clean_transcript: parse_transcript_clean(
            values.get("auto_clean_transcript"),
            &defaults.auto_clean_transcript,
        ),
        http_server_enabled: parse_bool(
            values.get("http_server_enabled"),
            defaults.http_server_enabled,
        ),
        http_server_port: http_port,
        http_server_token: if http_token_needs_generation {
            generate_http_token()
        } else {
            http_token
        },
        default_summarize: parse_bool(values.get("default_summarize"), defaults.default_summarize),
        default_summarize_mode: parse_summarize_mode(
            values.get("default_summarize_mode"),
            &defaults.default_summarize_mode,
        ),
        summarize_model: parse_summarize_backend(
            values.get("summarize_model"),
            &defaults.summarize_model,
        ),
        claude_model_effort: parse_claude_model_effort(
            // New canonical key, with fallback to the v9 `summarize_effort` value
            // so an existing config.ini doesn't lose its effort setting on upgrade.
            values
                .get("claude_model_effort")
                .or_else(|| values.get("summarize_effort")),
            &defaults.claude_model_effort,
        ),
    };

    // Every directory in config.ini is meant to be absolute (storage, ffmpeg,
    // whisperx already are). Self-heal a relative temp_directory by anchoring it
    // to the config file's directory: this matches the code default and, more
    // importantly, keeps frozen config snapshots valid no matter where the
    // snapshot .ini is written (the backend resolves a relative temp_directory
    // against the config file's own parent, which for a snapshot is wrong).
    if !settings.temp_directory.trim().is_empty()
        && PathBuf::from(&settings.temp_directory).is_relative()
    {
        settings.temp_directory = base
            .join(&settings.temp_directory)
            .to_string_lossy()
            .into_owned();
    }

    // Persist the auto-generated token the first time we mint one so the user
    // sees it in config.ini and the Firefox extension can be configured with
    // the same value.
    if http_token_needs_generation {
        let _ = save_config_file(&settings);
    }

    Ok(settings)
}

// Read the current live working-state keys straight off disk, falling back to
// the default_* settings for any key not yet present. Used to preserve the
// main-window toggle state whenever the file is rewritten from the Settings
// dialog or created from scratch.
fn read_live_modes_from_disk(settings: &SettingsPayload) -> LiveModes {
    let fallback = LiveModes::from_default_settings(settings);
    let Ok(content) = ({
        let _guard = config_guard();
        fs::read_to_string(config_path())
    }) else {
        return fallback;
    };
    let values = parse_ini_default_section(&content);
    LiveModes {
        video: parse_bool(values.get("video"), fallback.video),
        audio: parse_bool(values.get("audio"), fallback.audio),
        transcript: parse_bool(values.get("transcript"), fallback.transcript),
        summarize: parse_bool(values.get("summarize"), fallback.summarize),
        verbose: parse_bool(values.get("verbose"), fallback.verbose),
    }
}

fn config_file_contents(settings: &SettingsPayload, live: &LiveModes) -> String {
    let lines = [
        "[DEFAULT]".to_owned(),
        format!("storage_directory={}", settings.storage_directory),
        format!("temp_directory={}", settings.temp_directory),
        format!("filename_template={}", settings.filename_template),
        format!("ffmpeg_path={}", settings.ffmpeg_path),
        format!("whisperx_path={}", settings.whisperx_path),
        format!("browser={}", settings.browser),
        format!("browser_path={}", settings.browser_path),
        format!("default_model={}", settings.default_model),
        format!("default_verbose={}", settings.default_verbose),
        format!("default_video={}", settings.default_video),
        format!("default_audio={}", settings.default_audio),
        format!("default_transcript={}", settings.default_transcript),
        format!("default_keep={}", settings.default_keep),
        format!("default_clips_full_output={}", settings.default_clips_full_output),
        format!(
            "default_transcript_source={}",
            settings.default_transcript_source
        ),
        format!("auto_clean_transcript={}", settings.auto_clean_transcript),
        format!("http_server_enabled={}", settings.http_server_enabled),
        format!("http_server_port={}", settings.http_server_port),
        format!("http_server_token={}", settings.http_server_token),
        format!("default_summarize={}", settings.default_summarize),
        format!("default_summarize_mode={}", settings.default_summarize_mode),
        format!("summarize_model={}", settings.summarize_model),
        format!("claude_model_effort={}", settings.claude_model_effort),
        // Live working-state mirror of the main-window checkboxes (see
        // LiveModes). Written instantly on every toggle; the GUI seeds the
        // checkboxes from the default_* keys above on startup and never reads
        // these back.
        format!("video={}", live.video),
        format!("audio={}", live.audio),
        format!("transcript={}", live.transcript),
        format!("summarize={}", live.summarize),
        format!("verbose={}", live.verbose),
    ];

    lines.join("\n") + "\n"
}

fn write_config_file(settings: &SettingsPayload, live: &LiveModes) -> Result<(), String> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            format!(
                "Failed to create config directory at {}: {err}",
                parent.display()
            )
        })?;
    }

    let _guard = config_guard();
    fs::write(&path, config_file_contents(settings, live))
        .map_err(|err| format!("Failed to save config at {}: {err}", path.display()))
}

fn save_config_file(settings: &SettingsPayload) -> Result<(), String> {
    // Preserve the live working-state keys when rewriting from the Settings
    // dialog or on first creation, so saving the defaults never clobbers the
    // main-window toggle state (and vice versa).
    let live = read_live_modes_from_disk(settings);
    write_config_file(settings, &live)
}

type LogFile = Arc<Mutex<Option<fs::File>>>;

fn init_log_file(log_file: &LogFile, temp_dir: &Path) {
    let _ = fs::create_dir_all(temp_dir);
    let path = temp_dir.join("avtget_debug.log");
    let file = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .ok();
    if let Ok(mut guard) = log_file.lock() {
        *guard = file;
    }
}

fn write_log_line(log_file: &LogFile, line: &str) {
    if let Ok(mut guard) = log_file.lock() {
        if let Some(ref mut file) = *guard {
            let _ = file.write_all(line.as_bytes());
            let _ = file.write_all(b"\n");
            let _ = file.flush();
        }
    }
}

fn emit_backend_event(app: &AppHandle, payload: Value, log_file: &LogFile) {
    // Extract human-readable text for the log file
    if let Some(obj) = payload.as_object() {
        let event_type = obj.get("type").and_then(Value::as_str).unwrap_or("");
        let text = match event_type {
            "log" => obj.get("message").and_then(Value::as_str),
            "job_finished" => obj.get("summary").and_then(Value::as_str),
            "job_error" => obj.get("error").and_then(Value::as_str),
            _ => None,
        };
        if let Some(line) = text {
            write_log_line(log_file, line);
        }
    }
    let _ = app.emit("backend-event", payload);
}

fn emit_backend_log(app: &AppHandle, message: impl Into<String>, log_file: &LogFile) {
    let msg = message.into();
    write_log_line(log_file, &msg);
    let _ = app.emit("backend-event", json!({ "type": "log", "message": msg }));
}

fn resolve_python_executable() -> String {
    if let Ok(path) = env::var("AVTGET_PYTHON_EXE") {
        let trimmed = path.trim();
        if !trimmed.is_empty() && PathBuf::from(trimmed).exists() {
            return trimmed.to_owned();
        }
    }

    let base = app_base_path();
    let candidates = [
        base.join("backend").join("python.exe"),
        base.join("backend").join("dist").join("python.exe"),
    ];
    for path in candidates {
        if path.exists() {
            return path.to_string_lossy().into_owned();
        }
    }
    "python".to_owned()
}

fn resolve_optional_path(paths: &[PathBuf]) -> Option<String> {
    paths
        .iter()
        .find(|path| path.exists())
        .map(|path| path.to_string_lossy().into_owned())
}

fn resolve_backend_executable() -> Option<PathBuf> {
    if let Ok(path) = env::var("AVTGET_BACKEND_EXE") {
        let candidate = PathBuf::from(path.trim());
        if candidate.exists() {
            return Some(candidate);
        }
    }

    let base = app_base_path();
    let repo = repo_root_path();
    let candidates = [
        base.join("backend").join("avtget-backend.exe"),
        base.join("backend").join("dist").join("avtget-backend.exe"),
        base.join("rust")
            .join("target")
            .join("release")
            .join("avtget-backend.exe"),
        base.join("rust")
            .join("target")
            .join("debug")
            .join("avtget-backend.exe"),
        repo.join("rust")
            .join("target")
            .join("release")
            .join("avtget-backend.exe"),
        repo.join("rust")
            .join("target")
            .join("debug")
            .join("avtget-backend.exe"),
    ];

    candidates.into_iter().find(|path| path.exists())
}

fn stream_backend_stdout(stdout: impl std::io::Read, app: AppHandle, log_file: LogFile) {
    let reader = BufReader::new(stdout);
    for line_result in reader.lines() {
        let Ok(line) = line_result else {
            continue;
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(trimmed) {
            Ok(payload) => emit_backend_event(&app, payload, &log_file),
            Err(_) => emit_backend_log(&app, line, &log_file),
        }
    }
}

fn stream_backend_stderr(stderr: impl std::io::Read, app: AppHandle, log_file: LogFile) {
    let reader = BufReader::new(stderr);
    for line_result in reader.lines() {
        let Ok(line) = line_result else {
            continue;
        };
        if line.trim().is_empty() {
            continue;
        }
        emit_backend_log(&app, line, &log_file);
    }
}

fn monitor_backend_exit(app: AppHandle, state: Arc<Mutex<Option<ManagedProcess>>>, log_file: LogFile) {
    thread::spawn(move || {
        loop {
            // Short poll so the guard clears promptly after the backend
            // exits. Once the guard is cleared, `backend_exited` is emitted
            // below — the frontend drains queued Firefox intakes on that
            // event, so the process slot is guaranteed free before the
            // drain's start_job fires.
            thread::sleep(Duration::from_millis(25));
            let mut guard = match state.lock() {
                Ok(guard) => guard,
                Err(_) => break,
            };
            let Some(process) = guard.as_mut() else {
                break;
            };
            match process.child.try_wait() {
                Ok(Some(status)) => {
                    if !status.success() {
                        emit_backend_event(
                            &app,
                            json!({
                                "type": "job_error",
                                "error": format!(
                                    "Process exited with code {}",
                                    status
                                        .code()
                                        .map_or_else(|| "unknown".to_owned(), |code| code.to_string())
                                )
                            }),
                            &log_file,
                        );
                    }
                    let snapshot = process.config_snapshot.take();
                    *guard = None;
                    drop(guard);
                    // The per-job config snapshot has served its purpose now that
                    // the process is gone — remove it so avtget_temp doesn't
                    // accumulate snapshot files over a session.
                    if let Some(snapshot) = snapshot {
                        let _ = fs::remove_file(&snapshot);
                    }
                    // Process slot is free — release the lock first, then
                    // tell the frontend it can start the next queued intake.
                    emit_backend_event(
                        &app,
                        json!({ "type": "backend_exited" }),
                        &log_file,
                    );
                    break;
                }
                Ok(None) => {}
                Err(err) => {
                    emit_backend_event(
                        &app,
                        json!({
                            "type": "job_error",
                            "error": format!("Failed checking backend status: {err}")
                        }),
                        &log_file,
                    );
                    let snapshot = process.config_snapshot.take();
                    *guard = None;
                    drop(guard);
                    // The per-job config snapshot has served its purpose now that
                    // the process is gone — remove it so avtget_temp doesn't
                    // accumulate snapshot files over a session.
                    if let Some(snapshot) = snapshot {
                        let _ = fs::remove_file(&snapshot);
                    }
                    // Process slot is free — release the lock first, then
                    // tell the frontend it can start the next queued intake.
                    emit_backend_event(
                        &app,
                        json!({ "type": "backend_exited" }),
                        &log_file,
                    );
                    break;
                }
            }
        }
    });
}

fn resolve_storage_directory(storage_directory: &str) -> PathBuf {
    let candidate = PathBuf::from(storage_directory);
    if candidate.is_absolute() {
        return candidate;
    }
    let base = config_path()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join(candidate)
}

fn find_file_by_stem(
    directory: &Path,
    filestem: &str,
    preferred_extensions: &[&str],
) -> Option<PathBuf> {
    if !directory.exists() {
        return None;
    }

    let mut matches = Vec::new();
    let entries = fs::read_dir(directory).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let stem_matches = path
            .file_stem()
            .and_then(OsStr::to_str)
            .map(|stem| stem == filestem)
            .unwrap_or(false);
        if stem_matches {
            matches.push(path);
        }
    }

    if matches.is_empty() {
        return None;
    }

    for ext in preferred_extensions {
        if let Some(path) = matches.iter().find(|path| {
            path.extension()
                .and_then(OsStr::to_str)
                .map(|value| value.eq_ignore_ascii_case(ext.trim_start_matches('.')))
                .unwrap_or(false)
        }) {
            return Some(path.clone());
        }
    }

    matches.into_iter().next()
}

fn resolve_artifact_path(settings: &SettingsPayload, args: &ArtifactArgs) -> Option<PathBuf> {
    let artifact = args.artifact.trim();
    let filestem = args.filestem.trim();
    if artifact.is_empty() || filestem.is_empty() {
        return None;
    }

    let storage_dir = resolve_storage_directory(&settings.storage_directory);
    let (subdir, preferred_extensions): (&str, &[&str]) = match artifact {
        "video" => ("video", &[".mp4", ".mkv", ".webm", ".mov", ".avi"]),
        "audio" => ("audio", &[".mp3"]),
        "transcript" => ("transcripts", &[".txt", ".vtt", ".srt", ".ass"]),
        // Summary is written next to the transcript by the summarize subprocess
        // as `{filestem}-summarized.txt`.
        "summary" => {
            let candidate = storage_dir
                .join("transcripts")
                .join(format!("{filestem}-summarized.txt"));
            return candidate.exists().then_some(candidate);
        }
        _ => return None,
    };

    let directory = storage_dir.join(subdir);
    if artifact == "audio" {
        let candidate = directory.join(format!("{filestem}.mp3"));
        if candidate.exists() {
            return Some(candidate);
        }
    } else if artifact == "transcript" {
        let candidate = directory.join(format!("{filestem}.txt"));
        if candidate.exists() {
            return Some(candidate);
        }
    }

    find_file_by_stem(&directory, filestem, preferred_extensions)
}

fn configure_file_dialog(options: &Value) -> rfd::FileDialog {
    let mut dialog = rfd::FileDialog::new();

    if let Some(title) = options.get("title").and_then(Value::as_str) {
        dialog = dialog.set_title(title);
    }

    if let Some(default_path) = options.get("defaultPath").and_then(Value::as_str) {
        let path = PathBuf::from(default_path);
        if path.is_dir() {
            dialog = dialog.set_directory(path);
        } else {
            if let Some(parent) = path.parent() {
                dialog = dialog.set_directory(parent);
            }
            if let Some(file_name) = path.file_name().and_then(OsStr::to_str) {
                dialog = dialog.set_file_name(file_name);
            }
        }
    }

    if let Some(filters) = options.get("filters").and_then(Value::as_array) {
        for filter in filters {
            let Some(name) = filter.get("name").and_then(Value::as_str) else {
                continue;
            };
            let extensions = filter
                .get("extensions")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(|ext| ext.trim_start_matches('.').to_owned())
                        .filter(|ext| ext != "*")
                        .collect::<Vec<String>>()
                })
                .unwrap_or_default();
            if extensions.is_empty() {
                continue;
            }
            let extension_refs: Vec<&str> = extensions.iter().map(String::as_str).collect();
            dialog = dialog.add_filter(name, &extension_refs);
        }
    }

    dialog
}

fn clean_temp_directory() {
    let settings = match load_or_create_config() {
        Ok(settings) => settings,
        Err(err) => {
            eprintln!("[Startup] Failed loading config for temp cleanup: {err}");
            return;
        }
    };

    if settings.default_keep {
        println!("[Startup] default_keep=true; skipping temp cleanup");
        return;
    }

    // Resolve temp_directory relative to the config file's parent (project
    // root). When the user launches via the pinned shortcut, the process cwd
    // is NOT the project root, so a bare `PathBuf::from("avtget_temp")` would
    // resolve to a non-existent path and the cleanup would silently bail.
    let config_path = config_path();
    let program_dir = config_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let temp_dir = {
        let candidate = PathBuf::from(&settings.temp_directory);
        if candidate.is_absolute() {
            candidate
        } else {
            program_dir.join(candidate)
        }
    };
    if !temp_dir.exists() {
        println!(
            "[Startup] temp directory does not exist (nothing to clean): {}",
            temp_dir.display()
        );
        return;
    }

    let entries = match fs::read_dir(&temp_dir) {
        Ok(entries) => entries,
        Err(err) => {
            eprintln!(
                "[Startup] Failed reading temp directory {}: {err}",
                temp_dir.display()
            );
            return;
        }
    };

    let mut deleted = 0u32;
    let mut failed = 0u32;
    for entry in entries.flatten() {
        let path = entry.path();
        let result = if path.is_dir() {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        };
        match result {
            Ok(()) => deleted += 1,
            Err(err) => {
                eprintln!("[Startup] Failed deleting {}: {err}", path.display());
                failed += 1;
            }
        }
    }
    println!(
        "[Startup] Cleaned temp directory {}: {deleted} removed, {failed} failed",
        temp_dir.display()
    );
}

#[tauri::command]
fn start_job(
    config: Value,
    app: AppHandle,
    state: State<'_, BackendState>,
) -> Result<(), String> {
    // Wait briefly for any previous backend to finish exiting. The exit
    // monitor clears the process guard only after observing the actual exit,
    // which lags the backend's final `job_finished` event — usually by
    // milliseconds, but Python/CUDA teardown or a Dropbox file lock can
    // stretch it past any fixed window. Cover the common sub-second lag
    // here; if the slot still hasn't cleared after 2 s, fail loudly with an
    // Err so the caller can recover: the frontend re-queues external intakes
    // (retried on `backend_exited`) and surfaces failed manual starts in the
    // log.
    let wait_start = Instant::now();
    let mut guard = loop {
        let guard = state
            .process
            .lock()
            .map_err(|_| "Backend process state lock is poisoned".to_owned())?;
        if guard.is_none() || wait_start.elapsed() >= Duration::from_millis(2000) {
            break guard;
        }
        drop(guard);
        thread::sleep(Duration::from_millis(25));
    };
    if guard.is_some() {
        emit_backend_log(&app, "Backend already running", &state.log_file);
        // Exact string matched by the frontend's intake-requeue path
        // (externalSubmit.ts) — keep in sync.
        return Err("Backend already running".to_owned());
    }

    let config_path = config_path();
    let program_dir = config_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    let mut object = config
        .as_object()
        .cloned()
        .ok_or_else(|| "start_job expected an object payload".to_owned())?;
    let should_set_program_dir = object
        .get("program_dir")
        .and_then(Value::as_str)
        .map(|value| value.trim().is_empty())
        .unwrap_or(true);
    if should_set_program_dir {
        object.insert(
            "program_dir".to_owned(),
            Value::String(program_dir.to_string_lossy().into_owned()),
        );
    }

    // Per-job frozen config snapshot (written by freeze_config at GO time). When
    // present, the spawned backend reads it via AVTGET_CONFIG_PATH instead of the
    // live config.ini, so a queued/running job is immune to later setting edits.
    // Pulled out of the payload so it isn't forwarded into the backend's JobConfig.
    let config_snapshot_path = object
        .remove("config_snapshot_path")
        .as_ref()
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let child_config_path = config_snapshot_path
        .clone()
        .unwrap_or_else(|| config_path.clone());

    let job_config_json =
        serde_json::to_string(&Value::Object(object)).map_err(|err| err.to_string())?;
    let backend_executable = resolve_backend_executable()
        .ok_or_else(|| "Could not locate avtget-backend.exe".to_owned())?;

    let python_executable = resolve_python_executable();
    let whisper_bridge = resolve_optional_path(&[
        app_base_path().join("backend").join("whisper_bridge.py"),
        repo_root_path().join("backend").join("whisper_bridge.py"),
    ]);

    let mut args = vec![
        "--job-config".to_owned(),
        job_config_json,
        "--python-executable".to_owned(),
        python_executable.clone(),
    ];
    if let Some(whisper_bridge) = whisper_bridge {
        args.push("--python-whisper-bridge".to_owned());
        args.push(whisper_bridge);
    }

    let log_file = state.log_file.clone();

    emit_backend_log(
        &app,
        format!(
            "Starting backend: {}",
            backend_executable.to_string_lossy()
        ),
        &log_file,
    );
    emit_backend_log(
        &app,
        format!("Program dir: {}", program_dir.to_string_lossy()),
        &log_file,
    );
    emit_backend_log(
        &app,
        format!("Config path: {}", child_config_path.to_string_lossy()),
        &log_file,
    );

    let mut command = Command::new(&backend_executable);
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("AVTGET_CONFIG_PATH", child_config_path.to_string_lossy().to_string());
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);

    let mut child = command
        .spawn()
        .map_err(|err| format!("Failed to start backend process: {err}"))?;

    if child.id() == 0 {
        return Err("Failed to start backend process".to_owned());
    }

    if let Some(stdout) = child.stdout.take() {
        let app_for_stdout = app.clone();
        let log_for_stdout = log_file.clone();
        thread::spawn(move || stream_backend_stdout(stdout, app_for_stdout, log_for_stdout));
    }
    if let Some(stderr) = child.stderr.take() {
        let app_for_stderr = app.clone();
        let log_for_stderr = log_file.clone();
        thread::spawn(move || stream_backend_stderr(stderr, app_for_stderr, log_for_stderr));
    }

    *guard = Some(ManagedProcess {
        child,
        config_snapshot: config_snapshot_path,
    });
    drop(guard);

    monitor_backend_exit(app, state.process.clone(), log_file);

    Ok(())
}

#[tauri::command]
fn cancel_job(state: State<'_, BackendState>) -> Result<(), String> {
    {
        let mut guard = state
            .process
            .lock()
            .map_err(|_| "Backend process state lock is poisoned".to_owned())?;
        let Some(process) = guard.as_mut() else {
            return Ok(());
        };

        if let Some(stdin) = process.child.stdin.as_mut() {
            let _ = stdin.write_all(br#"{"action":"cancel"}"#);
            let _ = stdin.write_all(b"\n");
            let _ = stdin.flush();
        }
    }

    let process_state = state.process.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(1));
        if let Ok(mut guard) = process_state.lock() {
            if let Some(process) = guard.as_mut() {
                let _ = process.child.kill();
            }
        }
    });

    Ok(())
}

// ---------------------------------------------------------------------------
// Ollama availability check
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
struct OllamaStatus {
    installed: bool,
    running: bool,
}

fn ping_ollama() -> bool {
    use std::net::TcpStream;
    TcpStream::connect_timeout(
        &"127.0.0.1:11434"
            .parse()
            .expect("valid socket addr"),
        Duration::from_secs(2),
    )
    .is_ok()
}

fn find_ollama_binary() -> Option<PathBuf> {
    let candidates = [
        Some(PathBuf::from(r"D:\Ollama\ollama.exe")),
        env::var("LOCALAPPDATA")
            .ok()
            .map(|local| PathBuf::from(local).join(r"Programs\Ollama\ollama.exe")),
    ];
    for candidate in candidates.into_iter().flatten() {
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    if let Ok(path_var) = env::var("PATH") {
        for dir in path_var.split(';') {
            let candidate = PathBuf::from(dir).join("ollama.exe");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

#[tauri::command]
fn check_ollama_available() -> OllamaStatus {
    OllamaStatus {
        installed: find_ollama_binary().is_some(),
        running: ping_ollama(),
    }
}

// ---------------------------------------------------------------------------

#[tauri::command]
fn get_config() -> Result<SettingsPayload, String> {
    load_or_create_config()
}

#[tauri::command]
fn save_config(settings: SettingsPayload) -> Result<(), String> {
    save_config_file(&settings)
}

// Immediate, write-only mirror of the five main-window checkboxes into
// config.ini. Reads the current settings (defaults, paths, etc.) and rewrites
// the file with the new live toggle state, leaving everything else untouched.
#[tauri::command]
fn set_live_modes(modes: LiveModes) -> Result<(), String> {
    let settings = load_or_create_config()?;
    write_config_file(&settings, &modes)
}

// Freeze the current config.ini into a per-job snapshot so the settings that
// govern a job are locked in at GO time and can't be mutated by a later toggle
// or Settings-Save while the job waits in the queue. The spawned backend reads
// this snapshot via AVTGET_CONFIG_PATH (see start_job); every path in it is
// absolute, so it resolves correctly regardless of the snapshot's own location.
// Regenerated from the parsed settings (not a raw copy) so it's always a
// complete, well-formed file. Returns the absolute snapshot path; the snapshot
// is deleted when its job's backend process exits.
#[tauri::command]
fn freeze_config() -> Result<String, String> {
    let settings = load_or_create_config()?;
    let live = read_live_modes_from_disk(&settings);
    let snapshot_dir = resolve_temp_dir(&settings).join("config-snapshots");
    fs::create_dir_all(&snapshot_dir).map_err(|err| {
        format!(
            "Failed to create config snapshot directory at {}: {err}",
            snapshot_dir.display()
        )
    })?;
    let dest = snapshot_dir.join(format!("config-{}.ini", next_snapshot_id()));
    fs::write(&dest, config_file_contents(&settings, &live))
        .map_err(|err| format!("Failed to write config snapshot at {}: {err}", dest.display()))?;
    Ok(dest.to_string_lossy().into_owned())
}

#[tauri::command]
fn log_message(message: String, state: State<'_, BackendState>) {
    write_log_line(&state.log_file, &message);
}

#[tauri::command]
fn show_open_dialog(options: Value) -> Option<String> {
    let properties = options
        .get("properties")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let open_directory = properties.iter().any(|value| value.as_str() == Some("openDirectory"));

    let dialog = configure_file_dialog(&options);
    if open_directory {
        dialog
            .pick_folder()
            .map(|path| path.to_string_lossy().into_owned())
    } else {
        dialog
            .pick_file()
            .map(|path| path.to_string_lossy().into_owned())
    }
}

#[tauri::command]
fn show_save_dialog(options: Value) -> SaveDialogResponse {
    let dialog = configure_file_dialog(&options);
    let file_path = dialog
        .save_file()
        .map(|path| path.to_string_lossy().into_owned());
    SaveDialogResponse {
        canceled: file_path.is_none(),
        file_path,
    }
}

#[tauri::command]
fn get_index_entry(_video_id: String) -> Option<Value> {
    None
}

#[tauri::command]
fn reload_index() {}

#[tauri::command]
fn read_text_file(file_path: String) -> Option<String> {
    fs::read_to_string(file_path).ok()
}

#[tauri::command]
fn write_text_file(file_path: String, content: String) -> bool {
    let path = PathBuf::from(&file_path);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    fs::write(path, content).is_ok()
}

#[tauri::command]
fn reveal_artifact(args: ArtifactArgs) -> bool {
    let settings = match load_or_create_config() {
        Ok(settings) => settings,
        Err(_) => return false,
    };

    let Some(target_path) = resolve_artifact_path(&settings, &args) else {
        return false;
    };

    #[cfg(windows)]
    {
        if Command::new("explorer")
            .arg("/select,")
            .arg(&target_path)
            .spawn()
            .is_ok()
        {
            true
        } else {
            target_path
                .parent()
                .map(|path| Command::new("explorer").arg(path).spawn().is_ok())
                .unwrap_or(false)
        }
    }
    #[cfg(not(windows))]
    {
        target_path
            .parent()
            .map(|path| open::that(path).is_ok())
            .unwrap_or(false)
    }
}

#[tauri::command]
fn open_artifact(args: ArtifactArgs) -> bool {
    let settings = match load_or_create_config() {
        Ok(settings) => settings,
        Err(_) => return false,
    };

    let Some(target_path) = resolve_artifact_path(&settings, &args) else {
        return false;
    };

    open::that(target_path).is_ok()
}

// Transcript cleaning (Claude) and summarization (Claude / Ollama) live in the
// Rust backend (rust/crates/backend/src/postprocess.rs). The backend
// orchestration loop drives clean → summarize sequentially per item, so the
// Tauri shell no longer needs Tauri commands for those operations.


// Expand a YouTube playlist URL into its component video URLs via yt-dlp.
// Runs synchronously on the Tauri command thread — typical latency is 1–3s.
// Returns canonical `https://www.youtube.com/watch?v=ID` strings.
#[tauri::command]
fn expand_playlist(url: String) -> Result<Vec<String>, String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err("expand_playlist called with empty URL".to_owned());
    }

    let args = [
        "--flat-playlist",
        "--print",
        "url",
        "--no-warnings",
        "--quiet",
        trimmed,
    ];

    let mut cmd = Command::new("yt-dlp");
    cmd.args(args);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    let output = match cmd.output() {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let python = resolve_python_executable();
            let mut fallback = Command::new(&python);
            fallback.arg("-m").arg("yt_dlp").args(args);
            #[cfg(windows)]
            fallback.creation_flags(CREATE_NO_WINDOW);
            fallback
                .output()
                .map_err(|e| format!("failed launching yt-dlp fallback: {e}"))?
        }
        Err(err) => return Err(format!("failed launching yt-dlp: {err}")),
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("yt-dlp playlist expansion failed: {}", stderr.trim()));
    }

    let urls: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect();

    if urls.is_empty() {
        return Err("Playlist expansion returned no URLs".to_owned());
    }

    Ok(urls)
}

// ---------------------------------------------------------------------------
// Local HTTP intake server
//
// Listens on 127.0.0.1:<http_server_port> and accepts job submissions from the
// companion Firefox extension. Each valid POST /jobs is forwarded to the
// frontend as an `external-job-request` Tauri event, which the React layer
// translates into a queued batch on the job store.
// ---------------------------------------------------------------------------

const HTTP_CORS_HEADERS: &[(&str, &str)] = &[
    ("Access-Control-Allow-Origin", "*"),
    ("Access-Control-Allow-Methods", "GET, POST, OPTIONS"),
    ("Access-Control-Allow-Headers", "Content-Type, Authorization"),
    ("Access-Control-Max-Age", "3600"),
];

fn cors_headers() -> Vec<tiny_http::Header> {
    HTTP_CORS_HEADERS
        .iter()
        .filter_map(|(name, value)| {
            tiny_http::Header::from_bytes(name.as_bytes(), value.as_bytes()).ok()
        })
        .collect()
}

fn build_response(status: u16, body: String) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
    let mut response = tiny_http::Response::from_string(body).with_status_code(status);
    for header in cors_headers() {
        response = response.with_header(header);
    }
    if let Ok(header) =
        tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
    {
        response = response.with_header(header);
    }
    response
}

fn json_error(status: u16, message: &str) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
    build_response(status, json!({ "error": message }).to_string())
}

fn preset_modes(preset: &str) -> Option<(bool, bool, bool, bool)> {
    // (video, audio, transcript, summarize)
    match preset {
        "archive_video" => Some((true, true, false, false)),
        "save_audio" => Some((false, true, false, false)),
        "save_transcript" => Some((false, false, true, false)),
        "summarize" => Some((false, false, true, true)),
        _ => None,
    }
}

fn extract_bearer_token(request: &tiny_http::Request) -> Option<String> {
    request.headers().iter().find_map(|header| {
        if header.field.equiv("Authorization") {
            let value = header.value.as_str();
            value
                .strip_prefix("Bearer ")
                .or_else(|| value.strip_prefix("bearer "))
                .map(|token| token.trim().to_owned())
        } else {
            None
        }
    })
}

fn handle_http_request(
    mut request: tiny_http::Request,
    app: &AppHandle,
    expected_token: &str,
) -> std::io::Result<()> {
    let method = request.method().clone();
    let url = request.url().to_owned();

    // CORS preflight — always answer OK, headers attached via build_response.
    if method == tiny_http::Method::Options {
        let response = build_response(200, "{}".to_owned());
        return request.respond(response);
    }

    // Health check — no auth required so the extension can probe availability.
    if method == tiny_http::Method::Get && (url == "/health" || url.starts_with("/health?")) {
        let body = json!({
            "status": "ok",
            "app": "avtget",
            "version": env!("CARGO_PKG_VERSION"),
        })
        .to_string();
        return request.respond(build_response(200, body));
    }

    if !(method == tiny_http::Method::Post && (url == "/jobs" || url.starts_with("/jobs?"))) {
        return request.respond(json_error(404, "not found"));
    }

    // Auth
    let provided = extract_bearer_token(&request).unwrap_or_default();
    if expected_token.is_empty() || provided != expected_token {
        return request.respond(json_error(401, "invalid or missing bearer token"));
    }

    // Read body (capped — no legitimate request is more than a few KB)
    const MAX_BODY: usize = 16 * 1024;
    let mut body = String::new();
    if let Err(err) = request
        .as_reader()
        .take(MAX_BODY as u64)
        .read_to_string(&mut body)
    {
        return request.respond(json_error(
            400,
            &format!("failed to read request body: {err}"),
        ));
    }

    let payload: Value = match serde_json::from_str(&body) {
        Ok(value) => value,
        Err(err) => {
            return request.respond(json_error(400, &format!("invalid json: {err}")));
        }
    };

    let url_arg = payload
        .get("url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let Some(job_url) = url_arg else {
        return request.respond(json_error(400, "missing 'url' field"));
    };

    let preset_name = payload
        .get("preset")
        .and_then(Value::as_str)
        .unwrap_or("archive_video");
    let Some((video, audio, transcript, summarize)) = preset_modes(preset_name) else {
        return request.respond(json_error(
            400,
            &format!("unknown preset '{preset_name}'"),
        ));
    };

    // Optional overrides (accept a handful of fields the user might want to
    // override per-request from the extension). Anything else is ignored.
    let overrides = payload.get("options").cloned().unwrap_or_else(|| json!({}));

    let event_payload = json!({
        "url": job_url,
        "preset": preset_name,
        "modes": {
            "video": video,
            "audio": audio,
            "transcript": transcript,
            "summarize": summarize,
        },
        "overrides": overrides,
    });

    let _ = app.emit("external-job-request", event_payload);

    let response_body = json!({
        "status": "queued",
        "url": job_url,
        "preset": preset_name,
    })
    .to_string();
    request.respond(build_response(202, response_body))
}

fn start_http_server(app: AppHandle) {
    let settings = match load_or_create_config() {
        Ok(settings) => settings,
        Err(err) => {
            eprintln!("[HttpServer] Failed loading config: {err}");
            return;
        }
    };

    if !settings.http_server_enabled {
        println!("[HttpServer] Disabled in config; not starting");
        return;
    }

    if settings.http_server_token.trim().is_empty() {
        eprintln!("[HttpServer] Not starting: http_server_token is empty");
        return;
    }

    let addr = format!("127.0.0.1:{}", settings.http_server_port);
    let server = match tiny_http::Server::http(addr.clone()) {
        Ok(server) => server,
        Err(err) => {
            eprintln!("[HttpServer] Failed to bind {addr}: {err}");
            return;
        }
    };

    println!("[HttpServer] Listening on http://{addr}");

    let token = settings.http_server_token.clone();
    thread::spawn(move || {
        for request in server.incoming_requests() {
            // Re-read the token on every request so that if the user rotates
            // it via the Settings dialog (which writes config.ini), the change
            // takes effect without restarting Avtget.
            let live_token = load_or_create_config()
                .map(|s| s.http_server_token)
                .unwrap_or_else(|_| token.clone());

            if let Err(err) = handle_http_request(request, &app, &live_token) {
                eprintln!("[HttpServer] Failed to send response: {err}");
            }
        }
    });
}

fn main() {
    tauri::Builder::default()
        .manage(BackendState::default())
        .setup(|app| {
            clean_temp_directory();

            let settings =
                load_or_create_config().unwrap_or_else(|_| default_settings(&repo_root_path()));
            let program_dir = config_path()
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."));
            let temp_dir = {
                let candidate = PathBuf::from(&settings.temp_directory);
                if candidate.is_absolute() {
                    candidate
                } else {
                    program_dir.join(candidate)
                }
            };
            init_log_file(&app.state::<BackendState>().log_file, &temp_dir);

            start_http_server(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            start_job,
            cancel_job,
            get_config,
            save_config,
            set_live_modes,
            freeze_config,
            log_message,
            check_ollama_available,
            show_open_dialog,
            show_save_dialog,
            get_index_entry,
            reload_index,
            read_text_file,
            write_text_file,
            reveal_artifact,
            open_artifact,
            expand_playlist
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
