use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use crate::events::EventEmitter;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(windows)]
const DETACHED_PROCESS: u32 = 0x0000_0008;

const OLLAMA_ADDR: &str = "127.0.0.1:11434";
const PING_TIMEOUT: Duration = Duration::from_secs(2);
const STARTUP_POLL_INTERVAL: Duration = Duration::from_secs(1);
const STARTUP_MAX_ATTEMPTS: u32 = 10;

/// Check if Ollama API is responding by attempting a TCP connection.
pub fn ping_ollama() -> bool {
    TcpStream::connect_timeout(
        &OLLAMA_ADDR.parse().expect("valid socket addr"),
        PING_TIMEOUT,
    )
    .is_ok()
}

/// Search for the Ollama binary in known locations and PATH.
pub fn find_ollama_binary() -> Option<PathBuf> {
    let candidates = [
        Some(PathBuf::from(r"D:\Ollama\ollama.exe")),
        std::env::var("LOCALAPPDATA")
            .ok()
            .map(|local| PathBuf::from(local).join(r"Programs\Ollama\ollama.exe")),
    ];

    for candidate in candidates.into_iter().flatten() {
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    // Search PATH
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in path_var.split(';') {
            let candidate = PathBuf::from(dir).join("ollama.exe");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    None
}

/// Ensure Ollama is running before transcript cleaning.
///
/// If the API is already responding, returns true immediately.
/// If not, attempts to find and start `ollama serve` as a detached process,
/// then polls until the API responds (up to ~10 seconds).
///
/// Returns false (never errors) if Ollama cannot be started — cleaning
/// should be skipped, not treated as a job failure.
pub fn ensure_ollama_running(emitter: &EventEmitter) -> bool {
    if ping_ollama() {
        let _ = emitter.emit_log("Ollama: API responding at localhost:11434");
        return true;
    }

    let _ = emitter.emit_log("Ollama: API not responding, attempting auto-start...");

    let Some(binary) = find_ollama_binary() else {
        let _ = emitter.emit_log(
            "Ollama: Binary not found (checked D:\\Ollama\\, %LOCALAPPDATA%\\Programs\\Ollama\\, PATH). \
             Skipping transcript cleaning.",
        );
        return false;
    };

    let _ = emitter.emit_log(format!(
        "Ollama: Found binary at {}, starting `ollama serve`...",
        binary.display()
    ));

    let mut cmd = Command::new(&binary);
    cmd.arg("serve")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);

    if let Err(e) = cmd.spawn() {
        let _ = emitter.emit_log(format!(
            "Ollama: Failed to launch 'ollama serve': {e}. Skipping transcript cleaning."
        ));
        return false;
    }

    for attempt in 1..=STARTUP_MAX_ATTEMPTS {
        thread::sleep(STARTUP_POLL_INTERVAL);
        if ping_ollama() {
            let _ = emitter.emit_log("Ollama: Server started successfully");
            return true;
        }
        if attempt % 3 == 0 {
            let _ = emitter.emit_log(format!(
                "Ollama: Waiting for server to start... ({attempt}s)"
            ));
        }
    }

    let _ = emitter.emit_log(format!(
        "Ollama: Server did not respond after {STARTUP_MAX_ATTEMPTS} seconds. \
         Skipping transcript cleaning."
    ));
    false
}
