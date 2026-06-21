use std::process::Command;

use avtget_domain::{BackendError, Result};
use serde_json::Value;

use super::{ChannelScrapeAdapter, ChannelScrapeRequest};

#[derive(Debug, Default)]
pub struct PythonChannelScrapeBridgeAdapter;

impl ChannelScrapeAdapter for PythonChannelScrapeBridgeAdapter {
    fn scrape_channel_urls(&self, request: ChannelScrapeRequest) -> Result<Vec<String>> {
        let mut command = Command::new(&request.python_executable);
        command
            .arg(&request.bridge_script)
            .arg("--channel-url")
            .arg(&request.channel_url)
            .arg("--timeframe-days")
            .arg(request.timeframe_days.to_string());

        command.arg("--browser").arg(&request.browser);
        if let Some(path) = request.browser_path.as_ref() {
            command.arg("--browser-path").arg(path);
        }
        if request.verbose {
            command.arg("--verbose");
        }

        let output = command.output().map_err(|err| {
            BackendError::Process(format!("failed to launch channel scrape bridge: {err}"))
        })?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let payload = parse_payload(&stdout).ok_or_else(|| {
            BackendError::Process("channel scrape bridge returned no JSON payload".to_owned())
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
                "channel scrape bridge failed (code {:?}): {} {}",
                output.status.code(),
                err_msg,
                stderr.trim()
            )));
        }

        let urls = payload
            .get("urls")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
                    .collect::<Vec<String>>()
            })
            .unwrap_or_default();
        Ok(urls)
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
