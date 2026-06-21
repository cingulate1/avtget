use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use avtget_domain::{BackendError, Result};
use serde_json::Value;

use super::{TranscriptCleanOutcome, TranscriptCleanRequest, TranscriptCleanerAdapter};

#[derive(Debug, Default)]
pub struct PythonTranscriptCleanBridgeAdapter;

impl TranscriptCleanerAdapter for PythonTranscriptCleanBridgeAdapter {
    fn clean_transcript(&self, request: TranscriptCleanRequest) -> Result<TranscriptCleanOutcome> {
        // The result payload is written to a temp file by the Python bridge
        // (instead of stdout) so that Python's verbose log events can flow
        // through stdout to the Tauri reader in real-time.
        let result_path = request.output_path.with_extension("result.json");

        // Clean up any stale result file
        let _ = fs::remove_file(&result_path);

        let mut command = Command::new(&request.python_executable);
        command
            .arg(&request.bridge_script)
            .arg("--input-path")
            .arg(&request.transcript_path)
            .arg("--output-path")
            .arg(&request.output_path)
            .arg("--result-path")
            .arg(&result_path)
            .arg("--cleaner")
            .arg(&request.cleaner)
            .env("PYTHONUNBUFFERED", "1");

        // Always pass --verbose so Python emits log events regardless of the
        // UI checkbox state.  The frontend filters display, not generation.
        command.arg("--verbose");

        // Pipe stdout so we can forward each line to both the parent process's
        // stdout (for Tauri) and the debug log file. Previously Stdio::inherit()
        // was used, which sent output to Tauri but bypassed the log file writer.
        command
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .stdin(Stdio::null());

        let mut child = command.spawn().map_err(|err| {
            BackendError::Process(format!("failed to launch transcript cleaner bridge: {err}"))
        })?;

        // Read Python's stdout line by line and forward to both our stdout
        // (which Tauri's BufReader consumes) and the debug log file.
        let child_stdout = child.stdout.take();
        if let Some(pipe) = child_stdout {
            let reader = BufReader::new(pipe);
            let stdout_handle = std::io::stdout();
            // Open log file for appending (if path provided)
            let mut log_file = request
                .log_file_path
                .as_ref()
                .and_then(|path| OpenOptions::new().create(true).append(true).open(path).ok());
            for line in reader.lines() {
                let line = line.map_err(|err| {
                    BackendError::Process(format!(
                        "failed reading transcript cleaner stdout: {err}"
                    ))
                })?;
                // Write to parent stdout (Tauri reads this)
                {
                    let mut out = stdout_handle.lock();
                    let _ = writeln!(out, "{line}");
                    let _ = out.flush();
                }
                // Mirror to debug log file
                if let Some(ref mut file) = log_file {
                    let _ = writeln!(file, "{line}");
                    let _ = file.flush();
                }
            }
        }

        let status = child.wait().map_err(|err| {
            BackendError::Process(format!(
                "failed to wait on transcript cleaner bridge: {err}"
            ))
        })?;

        // Read the result payload from the file the Python bridge wrote
        let result_json = fs::read_to_string(&result_path).map_err(|err| {
            BackendError::Process(format!(
                "transcript cleaner bridge did not write result file ({}): {}",
                result_path.display(),
                err,
            ))
        })?;
        let _ = fs::remove_file(&result_path);

        let payload: Value = serde_json::from_str(&result_json).map_err(|err| {
            BackendError::Process(format!(
                "transcript cleaner bridge result is not valid JSON: {err}"
            ))
        })?;

        let ok = payload
            .get("ok")
            .and_then(Value::as_bool)
            .unwrap_or(status.success());
        if !ok || !status.success() {
            let err_msg = payload
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            return Err(BackendError::Process(format!(
                "transcript cleaner bridge failed (code {:?}): {}",
                status.code(),
                err_msg,
            )));
        }

        let cleaner = payload
            .get("cleaner")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        let provider = payload
            .get("provider")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        let used_sharding = payload.get("used_sharding").and_then(Value::as_bool);
        let shards_total = payload
            .get("shards_total")
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok());
        let raw_chars = payload
            .get("raw_chars")
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok());
        let cleaned_chars = payload
            .get("cleaned_chars")
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok());

        Ok(TranscriptCleanOutcome {
            cleaner,
            provider,
            used_sharding,
            shards_total,
            raw_chars,
            cleaned_chars,
        })
    }
}
