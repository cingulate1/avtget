use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use avtget_domain::{BackendError, Result};
use serde_json::Value;

use super::{WhisperBridgeAdapter, WhisperBridgeRequest};

#[derive(Debug, Default)]
pub struct PythonWhisperBridgeAdapter;

impl WhisperBridgeAdapter for PythonWhisperBridgeAdapter {
    fn transcribe(&self, request: WhisperBridgeRequest) -> Result<()> {
        let clips_payload: Vec<Value> = request
            .clips
            .iter()
            .map(|clip| {
                serde_json::json!({
                    "start": clip.start,
                    "end": clip.end,
                })
            })
            .collect();

        let effective_python = derive_conda_python(&request.whisperx_path)
            .unwrap_or_else(|| request.python_executable.clone());

        let mut command = Command::new(&effective_python);
        command
            .arg(&request.bridge_script)
            .arg("--audio-path")
            .arg(&request.audio_path)
            .arg("--output-dir")
            .arg(&request.output_dir)
            .arg("--temp-dir")
            .arg(&request.temp_dir)
            .arg("--output-filestem")
            .arg(&request.output_filestem)
            .arg("--model")
            .arg(&request.model)
            .arg("--gpu")
            .arg(&request.gpu)
            .arg("--clips-json")
            .arg(serde_json::to_string(&clips_payload)?);

        if request.clips_full_output && !request.clips.is_empty() {
            command.arg("--clips-full-output");
        }
        if let Some(whisperx_path) = request.whisperx_path.as_ref() {
            command.arg("--whisperx-path").arg(whisperx_path);
        }
        if let Some(ffmpeg_path) = request.ffmpeg_path.as_ref() {
            command.arg("--ffmpeg-path").arg(ffmpeg_path);
        }

        command
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .stdin(Stdio::null());

        let mut child = command.spawn().map_err(|err| {
            BackendError::Process(format!("failed to launch whisper bridge: {err}"))
        })?;

        let mut stdout_text = String::new();
        if let Some(pipe) = child.stdout.take() {
            let reader = BufReader::new(pipe);
            let stdout_handle = std::io::stdout();
            for line in reader.lines() {
                let line = line.map_err(|err| {
                    BackendError::Process(format!("failed reading whisper bridge stdout: {err}"))
                })?;
                stdout_text.push_str(&line);
                stdout_text.push('\n');
                let is_payload_line = serde_json::from_str::<Value>(&line)
                    .ok()
                    .map(|value| value.get("ok").is_some() || value.get("error").is_some())
                    .unwrap_or(false);
                if is_payload_line {
                    continue;
                }
                let mut out = stdout_handle.lock();
                let _ = writeln!(out, "{line}");
                let _ = out.flush();
            }
        }

        let status = child.wait().map_err(|err| {
            BackendError::Process(format!("failed to wait on whisper bridge: {err}"))
        })?;
        let payload = parse_payload(&stdout_text);

        if !status.success() {
            let payload_error = payload
                .as_ref()
                .and_then(|value| value.get("error"))
                .and_then(Value::as_str)
                .unwrap_or("");
            return Err(BackendError::Process(format!(
                "whisper bridge exited with code {:?}: {}",
                status.code(),
                payload_error,
            )));
        }

        let ok = payload
            .as_ref()
            .and_then(|value| value.get("ok"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !ok {
            let reason = payload
                .as_ref()
                .and_then(|value| value.get("error"))
                .and_then(Value::as_str)
                .unwrap_or("unknown whisper bridge error");
            return Err(BackendError::Process(reason.to_owned()));
        }

        Ok(())
    }
}

/// Derive the conda environment's `python.exe` from a whisperx executable path.
///
/// `whisperx_path` is typically something like
/// `D:\Anaconda3\envs\whisperx\Scripts\whisperx.exe`.  Going up two levels
/// from the executable yields the environment root, where `python.exe` lives.
///
/// Returns `None` if the path is absent or the derived python does not exist.
fn derive_conda_python(whisperx_path: &Option<String>) -> Option<String> {
    let raw = whisperx_path.as_ref()?;
    let exe_path = Path::new(raw);
    let env_root = exe_path.parent()?.parent()?;
    let conda_python = env_root.join("python.exe");
    if conda_python.exists() {
        Some(conda_python.to_string_lossy().into_owned())
    } else {
        None
    }
}

fn parse_payload(stdout: &str) -> Option<Value> {
    for line in stdout.lines().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
            return Some(value);
        }
    }
    None
}
