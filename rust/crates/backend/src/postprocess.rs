//! Post-transcript processing: Claude transcript cleaning + transcript
//! summarization (Ollama HTTP API or `claude -p` subprocess).
//!
//! Lives in the backend so the orchestration loop drives clean → summarize
//! sequentially per item — the previous frontend-driven chain let the cleaning
//! step batch across all items before any summarize started, and emitted
//! `Job completed` mid-batch.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use serde_json::{json, Value};

use avtget_domain::{ArtifactKind, BackendError, BackendEvent, Result, Settings};

use crate::cancel::CancellationToken;
use crate::events::EventEmitter;
use crate::ollama::ensure_ollama_running;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const OLLAMA_SUMMARIZE_MODEL: &str = "gemma4:31b-cloud";

const SUMMARIZE_VALID_BACKENDS: &[&str] = &["claude", "ollama"];

const SUMMARIZE_SYSTEM_PROMPT: &str = "You are a transcript summarizer. Emit a thorough summary in flowing prose, using bullet points only when the content in the transcript itself is enumerated as a list. The transcript may be somewhat noisy from the process of automated speech recognition — focus on substance and ignore transcription artifacts. Start directly with the summary without any preamble, and do not append any concluding remarks.";

fn emit_log(emitter: &EventEmitter, message: impl Into<String>) {
    let _ = emitter.emit(BackendEvent::Log {
        message: message.into(),
    });
}

fn emit_summary_status(emitter: &EventEmitter, item_id: &str, status: &str) {
    let _ = emitter.emit(BackendEvent::ArtifactStatus {
        item_id: item_id.to_owned(),
        artifact: ArtifactKind::Summary,
        status: status.to_owned(),
    });
}

fn ensure_not_cancelled(cancel_token: &CancellationToken) -> Result<()> {
    if cancel_token.is_cancelled() {
        Err(BackendError::Cancelled)
    } else {
        Ok(())
    }
}

fn validated_summarize_backend(raw: &str) -> &'static str {
    if SUMMARIZE_VALID_BACKENDS.contains(&raw) {
        if raw == "ollama" {
            "ollama"
        } else {
            "claude"
        }
    } else {
        "claude"
    }
}

// No `validated_claude_model`: `claude` is invoked without --model so the
// user's saved Claude Code default applies. Effort levels the resolved model
// doesn't support fall back to the nearest supported one CLI-side.
fn validated_claude_effort(raw: &str) -> &'static str {
    match raw {
        "low" => "low",
        "high" => "high",
        "xhigh" => "xhigh",
        "max" => "max",
        _ => "medium",
    }
}

// Summarization now pins `num_ctx` to gemma4's maximum context window
// regardless of transcript length. The previous "next power of 2 above 4×
// estimated tokens" heuristic is preserved below for reference / re-enabling.
const SUMMARIZE_NUM_CTX: u64 = 262_144;

// /// Count tokens using tiktoken o200k_base encoding (Python parity).
// fn count_tokens_o200k(text: &str) -> u64 {
//     tiktoken_rs::o200k_base()
//         .map(|bpe| bpe.encode_with_special_tokens(text).len() as u64)
//         .unwrap_or_else(|_| (text.len() as u64 + 3) / 4)
// }
//
// /// Next power of 2 that is >= 4× the token count of the transcript.
// fn summarize_num_ctx(transcript: &str) -> u64 {
//     let tokens = count_tokens_o200k(transcript);
//     let minimum = tokens.saturating_mul(4).max(1);
//     minimum.next_power_of_two()
// }

fn build_summarize_claude_prompt(transcript_path: &Path, output_path: &Path) -> String {
    format!(
        "Summarize the transcript at `{}` and write the summary to `{}`. Thanks!!",
        transcript_path.display(),
        output_path.display()
    )
}

fn strip_command_creation_flags(cmd: &mut Command) {
    cmd.env_remove("CLAUDECODE");
    cmd.env_remove("CLAUDE_CODE_TEAM_NAME");
    cmd.env_remove("CLAUDE_CODE_PLAN_MODE_REQUIRED");
    cmd.env_remove("CLAUDE_CODE_TASK_LIST_ID");
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
}

/// Resolve the summary output path next to the transcript file:
/// `{stem}-summarized.txt`.
pub fn summary_output_path(transcript_path: &Path) -> PathBuf {
    let parent = transcript_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let stem = transcript_path
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or("transcript");
    parent.join(format!("{stem}-summarized.txt"))
}

/// `claude -p --output-format json` (CLI v2.1.x) returns a JSON array of
/// stream events, each carrying `session_id`. Older versions returned a
/// single result object. Handle both.
fn extract_session_id(payload: &Value) -> Option<String> {
    let pick = |v: &Value| -> Option<String> {
        v.get("session_id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    if let Some(arr) = payload.as_array() {
        arr.iter().rev().find_map(pick)
    } else {
        pick(payload)
    }
}

// ---------------------------------------------------------------------------
// Claude transcript cleaning (two-turn `claude -p --resume`)
//
// Turn 1: `claude -p "load clean-transcript" --output-format json …` →
//         parse `session_id` from the JSON response.
// Turn 2: `claude -p "<path>"` (single) or `"<yt>\n<whisper>"` (dual)
//         `--resume <session_id> --output-format json`.
//
// For dual-input ("both" mode) the skill consumes both source files and writes
// a merged file at `{stem}.txt`; for single-input it overwrites the source in
// place. All tools are allowed; effort comes from Settings, while the model is
// the user's saved Claude Code default (no --model is passed).
// ---------------------------------------------------------------------------

/// Run Claude cleaning on a transcript. Returns Ok(true) on success,
/// Ok(false) on a logical failure (logged + emitted as failed status),
/// Err(Cancelled) if the job was cancelled.
pub fn clean_transcript_with_claude(
    emitter: &EventEmitter,
    cancel_token: &CancellationToken,
    item_id: &str,
    filestem: &str,
    transcript_paths: &[PathBuf],
    effort: &str,
) -> Result<bool> {
    ensure_not_cancelled(cancel_token)?;

    if transcript_paths.is_empty() {
        emit_log(
            emitter,
            format!("Cleaning skipped for {filestem}: no transcript files resolved"),
        );
        return Ok(false);
    }

    let effort = validated_claude_effort(effort);
    let dual = transcript_paths.len() == 2;
    let mode_label = if dual { "dual" } else { "single" };

    emit_log(
        emitter,
        format!("Cleaning started for {filestem} ({mode_label}, effort={effort})"),
    );

    // ---- Turn 1: load the skill, capture session_id ----
    let mut t1 = Command::new("claude");
    t1.arg("-p")
        .arg("load clean-transcript")
        .arg("--effort")
        .arg(effort)
        .arg("--output-format")
        .arg("json")
        .arg("--permission-mode")
        .arg("bypassPermissions")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    strip_command_creation_flags(&mut t1);

    let session_id = match t1.output() {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            match serde_json::from_str::<Value>(&stdout) {
                Ok(payload) => match extract_session_id(&payload) {
                    Some(sid) => sid,
                    None => {
                        emit_log(
                            emitter,
                            "Cleaning failed: Turn 1 succeeded but session_id missing from JSON",
                        );
                        return Ok(false);
                    }
                },
                Err(err) => {
                    emit_log(
                        emitter,
                        format!("Cleaning failed: Turn 1 returned non-JSON output: {err}"),
                    );
                    return Ok(false);
                }
            }
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let code = out
                .status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "unknown".to_owned());
            emit_log(
                emitter,
                format!("Cleaning failed: Turn 1 (load skill) exited {code}: {stderr}"),
            );
            return Ok(false);
        }
        Err(err) => {
            emit_log(
                emitter,
                format!(
                    "Cleaning failed: failed to spawn 'claude' for Turn 1 — is Claude Code installed and on PATH? ({err})"
                ),
            );
            return Ok(false);
        }
    };

    ensure_not_cancelled(cancel_token)?;

    // ---- Turn 2: provide the path(s) via --resume ----
    let prompt = transcript_paths
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("\n");

    let mut t2 = Command::new("claude");
    t2.arg("-p")
        .arg(&prompt)
        .arg("--resume")
        .arg(&session_id)
        .arg("--output-format")
        .arg("json")
        .arg("--permission-mode")
        .arg("bypassPermissions")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    strip_command_creation_flags(&mut t2);

    match t2.output() {
        Ok(out) if out.status.success() => {
            // Dual-input merges into `{stem}.txt`; single overwrites in place.
            let canonical_path: PathBuf = if dual {
                let first = &transcript_paths[0];
                let parent = first
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| PathBuf::from("."));
                let raw_stem = first.file_stem().and_then(OsStr::to_str).unwrap_or("");
                let stem = raw_stem
                    .strip_suffix("-yt")
                    .or_else(|| raw_stem.strip_suffix("-whisper"))
                    .unwrap_or(raw_stem);
                parent.join(format!("{stem}.txt"))
            } else {
                transcript_paths[0].clone()
            };

            if canonical_path.exists() {
                emit_log(
                    emitter,
                    format!("Cleaning finished: {}", canonical_path.display()),
                );
                Ok(true)
            } else {
                emit_log(
                    emitter,
                    format!(
                        "Cleaning failed: Claude exited OK but expected cleaned file not found at {}",
                        canonical_path.display()
                    ),
                );
                Ok(false)
            }
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let code = out
                .status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "unknown".to_owned());
            emit_log(
                emitter,
                format!("Cleaning failed: Turn 2 (clean) exited {code}: {stderr}"),
            );
            Ok(false)
        }
        Err(err) => {
            emit_log(
                emitter,
                format!("Cleaning failed: failed to spawn 'claude' for Turn 2: {err}"),
            );
            Ok(false)
        }
    }
    .map(|ok| {
        let _ = item_id; // currently no per-item status for clean; caller emits transcript status
        ok
    })
}

// ---------------------------------------------------------------------------
// Transcript summarization
// ---------------------------------------------------------------------------

/// Run summarization on a single transcript (backend selected by
/// `settings.summarize_model`). Emits summary artifact_status running →
/// completed/failed, writes `{stem}-summarized.txt`, and returns Ok(true) on
/// success / Ok(false) on logical failure / Err(Cancelled) on cancel.
pub fn summarize_transcript(
    emitter: &EventEmitter,
    cancel_token: &CancellationToken,
    settings: &Settings,
    item_id: &str,
    filestem: &str,
    transcript_path: &Path,
    output_path: &Path,
) -> Result<bool> {
    ensure_not_cancelled(cancel_token)?;

    let backend = validated_summarize_backend(&settings.summarize_model);
    let effort = validated_claude_effort(&settings.claude_model_effort);

    emit_summary_status(emitter, item_id, "running");

    let ok = if backend == "ollama" {
        run_ollama_summarize(
            emitter,
            cancel_token,
            item_id,
            filestem,
            transcript_path,
            output_path,
        )?
    } else {
        run_claude_summarize(
            emitter,
            cancel_token,
            item_id,
            filestem,
            transcript_path,
            output_path,
            effort,
        )?
    };

    emit_summary_status(emitter, item_id, if ok { "completed" } else { "failed" });
    Ok(ok)
}

fn run_ollama_summarize(
    emitter: &EventEmitter,
    cancel_token: &CancellationToken,
    _item_id: &str,
    filestem: &str,
    transcript_path: &Path,
    output_path: &Path,
) -> Result<bool> {
    if !ensure_ollama_running(emitter) {
        emit_log(emitter, "Ollama is not available — cannot summarize");
        return Ok(false);
    }

    ensure_not_cancelled(cancel_token)?;

    let transcript = match fs::read_to_string(transcript_path) {
        Ok(t) => t,
        Err(e) => {
            emit_log(emitter, format!("Failed to read transcript: {e}"));
            return Ok(false);
        }
    };

    // Always use gemma4's max context — see SUMMARIZE_NUM_CTX comment above.
    let num_ctx = SUMMARIZE_NUM_CTX;
    // let num_ctx = summarize_num_ctx(&transcript);
    emit_log(
        emitter,
        format!(
            "Summarizing transcript for {filestem} via Ollama ({OLLAMA_SUMMARIZE_MODEL}, num_ctx={num_ctx})"
        ),
    );

    let prompt = format!(
        "Please summarize the following transcript:\n\n\
         <transcript>\n{transcript}\n</transcript>\n\n\
         Output your summary with no preamble or concluding remarks. Thanks!!"
    );

    let payload = json!({
        "model": OLLAMA_SUMMARIZE_MODEL,
        "messages": [
            { "role": "system", "content": SUMMARIZE_SYSTEM_PROMPT },
            { "role": "user", "content": prompt }
        ],
        "stream": false,
        "think": true,
        "options": {
            "temperature": 1.0,
            "top_k": 64,
            "top_p": 0.95,
            "num_ctx": num_ctx,
        }
    });

    let request_url = "http://localhost:11434/api/chat";
    let result = ureq::post(request_url)
        .set("Content-Type", "application/json")
        .send_string(&payload.to_string());

    let response = match result {
        Ok(resp) => resp,
        Err(ureq::Error::Status(400, resp)) => {
            // Retry without `think: true` if the model rejects it.
            let body = resp.into_string().unwrap_or_default();
            if body.contains("does not support thinking") {
                let payload_no_think = json!({
                    "model": OLLAMA_SUMMARIZE_MODEL,
                    "messages": [
                        { "role": "system", "content": SUMMARIZE_SYSTEM_PROMPT },
                        { "role": "user", "content": prompt }
                    ],
                    "stream": false,
                    "options": {
                        "temperature": 1.0,
                        "top_k": 64,
                        "top_p": 0.95,
                        "num_ctx": num_ctx,
                    }
                });
                match ureq::post(request_url)
                    .set("Content-Type", "application/json")
                    .send_string(&payload_no_think.to_string())
                {
                    Ok(r) => r,
                    Err(e) => {
                        emit_log(
                            emitter,
                            format!("Ollama API error (retry without think): {e}"),
                        );
                        return Ok(false);
                    }
                }
            } else {
                emit_log(emitter, format!("Ollama API error (400): {body}"));
                return Ok(false);
            }
        }
        Err(e) => {
            emit_log(emitter, format!("Ollama API error: {e}"));
            return Ok(false);
        }
    };

    let body: Value = match response.into_json() {
        Ok(v) => v,
        Err(e) => {
            emit_log(emitter, format!("Failed to parse Ollama response: {e}"));
            return Ok(false);
        }
    };

    let content = body["message"]["content"]
        .as_str()
        .unwrap_or("")
        .trim()
        .to_owned();

    if content.is_empty() {
        emit_log(emitter, "Ollama returned empty summary");
        return Ok(false);
    }

    // Strip markdown code fences if the model wrapped its output.
    let final_content = {
        let lines: Vec<&str> = content.lines().collect();
        if lines.len() >= 2
            && lines[0].trim().starts_with("```")
            && lines[lines.len() - 1].trim().starts_with("```")
        {
            lines[1..lines.len() - 1].join("\n").trim().to_owned()
        } else {
            content
        }
    };

    match fs::write(output_path, &final_content) {
        Ok(()) => {
            emit_log(emitter, format!("Summary saved: {}", output_path.display()));
            Ok(true)
        }
        Err(e) => {
            emit_log(emitter, format!("Failed to write summary: {e}"));
            Ok(false)
        }
    }
}

fn run_claude_summarize(
    emitter: &EventEmitter,
    cancel_token: &CancellationToken,
    _item_id: &str,
    filestem: &str,
    transcript_path: &Path,
    output_path: &Path,
    effort: &str,
) -> Result<bool> {
    ensure_not_cancelled(cancel_token)?;

    emit_log(
        emitter,
        format!("Summarization started for {filestem} (effort={effort})"),
    );

    let prompt = build_summarize_claude_prompt(transcript_path, output_path);

    let mut command = Command::new("claude");
    command
        .arg("-p")
        .arg(&prompt)
        .arg("--agent")
        .arg("avtget-transcript-summarizer")
        .arg("--effort")
        .arg(effort)
        .arg("--output-format")
        .arg("json")
        .arg("--max-turns")
        .arg("20")
        .arg("--permission-mode")
        .arg("bypassPermissions")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    strip_command_creation_flags(&mut command);

    let outcome = command.output();
    match outcome {
        Ok(out) if out.status.success() => {
            if output_path.exists() {
                emit_log(emitter, format!("Summary saved: {}", output_path.display()));
                Ok(true)
            } else {
                emit_log(
                    emitter,
                    format!(
                        "Claude exited OK but no summary file was written to {}",
                        output_path.display()
                    ),
                );
                Ok(false)
            }
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let code = out
                .status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "unknown".to_owned());
            emit_log(
                emitter,
                format!("Summarize failed: claude exited {code}: {stderr}"),
            );
            Ok(false)
        }
        Err(err) => {
            emit_log(
                emitter,
                format!("Failed to spawn 'claude' — is Claude Code installed and on PATH? ({err})"),
            );
            Ok(false)
        }
    }
}
