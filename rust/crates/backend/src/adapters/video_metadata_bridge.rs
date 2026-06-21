use std::process::Command;

use avtget_domain::{BackendError, Result};
use serde_json::Value;

use super::{VideoMetadata, VideoMetadataAdapter, VideoMetadataRequest};

#[derive(Debug, Default)]
pub struct PythonVideoMetadataBridgeAdapter;

impl VideoMetadataAdapter for PythonVideoMetadataBridgeAdapter {
    fn fetch_metadata(&self, request: VideoMetadataRequest) -> Result<VideoMetadata> {
        let mut command = Command::new(&request.python_executable);
        command
            .arg(&request.bridge_script)
            .arg("--url")
            .arg(&request.url);

        command.arg("--browser").arg(&request.browser);
        if let Some(path) = request.browser_path.as_ref() {
            command.arg("--browser-path").arg(path);
        }
        if request.verbose {
            command.arg("--verbose");
        }

        let output = command.output().map_err(|err| {
            BackendError::Process(format!("failed to launch video metadata bridge: {err}"))
        })?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let payload = parse_payload(&stdout).ok_or_else(|| {
            BackendError::Process("video metadata bridge returned no JSON payload".to_owned())
        })?;

        let ok = payload
            .get("ok")
            .and_then(Value::as_bool)
            .unwrap_or(output.status.success());
        if !ok || !output.status.success() {
            let err_msg = payload
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            return Err(BackendError::Process(format!(
                "video metadata bridge failed (code {:?}): {} {}",
                output.status.code(),
                err_msg,
                stderr.trim()
            )));
        }

        let title = payload
            .get("title")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        let channel = payload
            .get("channel")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);

        Ok(VideoMetadata { title, channel })
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
