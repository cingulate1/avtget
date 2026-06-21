#!/usr/bin/env python3
"""
Transcript cleaning bridge for the Rust backend.

Verbose output is written directly to the REAL stdout (inherited from the
Rust backend process) as JSON log events. The result payload is written
to a --result-path file. This approach bypasses Rust's pipe reading entirely,
so verbose lines stream in real-time to whatever reads the backend's stdout.
"""

from __future__ import annotations

import argparse
import io
import json
import sys
from contextlib import redirect_stderr
from pathlib import Path
from typing import List

SCRIPT_DIR = Path(__file__).parent
sys.path.insert(0, str(SCRIPT_DIR))

from avtget_core.transcript_cleaner import clean_transcript  # noqa: E402

# Grab a reference to the REAL stdout before anything replaces sys.stdout.
# All JSON log events are written here so they go directly to the inherited
# pipe handle (Rust backend → Tauri).
_REAL_STDOUT = sys.stdout


def _emit_log(message: str) -> None:
    """Write a JSON log event to the real stdout (not sys.stdout which may be replaced)."""
    _REAL_STDOUT.write(json.dumps({"type": "log", "message": message}) + "\n")
    _REAL_STDOUT.flush()


def _provider_from_cleaner(cleaner: str) -> str:
    value = (cleaner or "").strip().lower()
    if value in {"off", "0", "false", "no", "none", "disabled"}:
        return "off"
    # Claude is never dispatched through this Python bridge — the Rust backend
    # drives `claude -p --resume` against the clean-transcript skill in-process.
    # If a Claude selector somehow reaches this bridge, treat it as off.
    if value in {"claude", "anthropic", "1", "true", "yes", "on"}:
        return "off"
    return "ollama"


def main() -> int:
    parser = argparse.ArgumentParser(description="Transcript cleaner bridge for Rust backend")
    parser.add_argument("--input-path", required=True)
    parser.add_argument("--output-path", required=True)
    parser.add_argument("--result-path", required=True)
    parser.add_argument("--cleaner", default="off")
    parser.add_argument("--verbose", action="store_true")
    args = parser.parse_args()

    input_path = Path(args.input_path)
    output_path = Path(args.output_path)
    result_path = Path(args.result_path)

    def write_result(payload: dict) -> None:
        result_path.parent.mkdir(parents=True, exist_ok=True)
        result_path.write_text(json.dumps(payload), encoding="utf-8")

    if not input_path.exists():
        write_result({"ok": False, "error": f"transcript file not found: {input_path}"})
        return 1

    try:
        raw_text = input_path.read_text(encoding="utf-8")
    except Exception as exc:
        write_result({"ok": False, "error": f"failed reading transcript: {exc}"})
        return 1

    cleaner = args.cleaner or "off"
    provider = _provider_from_cleaner(cleaner)
    raw_chars = len(raw_text)

    shard_state = {"current": 0, "total": 1}

    def shard_callback(current: int, total: int) -> None:
        if total > shard_state["total"]:
            shard_state["total"] = int(total)
        if current > shard_state["current"]:
            shard_state["current"] = int(current)

    class _LogCapture:
        """Intercepts print() output from clean_transcript() and emits each
        complete line as a JSON log event on _REAL_STDOUT."""

        def __init__(self) -> None:
            self._buffer = ""
            self.lines: List[str] = []

        def write(self, data: str) -> int:
            self._buffer += data
            while "\n" in self._buffer:
                line, self._buffer = self._buffer.split("\n", 1)
                line = line.strip()
                if line:
                    self.lines.append(line)
                    # Write to _REAL_STDOUT, NOT sys.stdout (which is self!)
                    _emit_log(line)
            return len(data)

        def flush(self) -> None:
            if self._buffer.strip():
                line = self._buffer.strip()
                self.lines.append(line)
                _emit_log(line)
                self._buffer = ""

    # Always enable verbose so log events stream to the frontend regardless
    # of the UI checkbox state.  The frontend filters display, not generation.
    try:
        capture = _LogCapture()
        sys.stdout = capture
        err_buffer = io.StringIO()
        try:
            with redirect_stderr(err_buffer):
                cleaned_text = clean_transcript(
                    raw_text=raw_text,
                    cleaner=cleaner,
                    verbose=True,
                    shard_callback=shard_callback,
                    log_callback=None,
                    stop_event=None,
                )
        finally:
            sys.stdout = _REAL_STDOUT
    except Exception as exc:
        write_result({"ok": False, "error": str(exc)})
        return 1

    if not cleaned_text:
        stderr_output = err_buffer.getvalue().strip()
        detail = f"cleaner returned empty output"
        if stderr_output:
            detail += f" — stderr: {stderr_output}"
        write_result({"ok": False, "error": detail})
        return 1

    try:
        output_path.parent.mkdir(parents=True, exist_ok=True)
        output_path.write_text(cleaned_text, encoding="utf-8")
    except Exception as exc:
        write_result({"ok": False, "error": f"failed writing cleaned transcript: {exc}"})
        return 1

    used_sharding = shard_state["total"] > 1

    write_result({
        "ok": True,
        "cleaner": cleaner,
        "provider": provider,
        "used_sharding": used_sharding,
        "shards_total": shard_state["total"],
        "shards_processed": max(shard_state["current"], 1 if used_sharding else 0),
        "raw_chars": raw_chars,
        "cleaned_chars": len(cleaned_text),
        "output_path": str(output_path),
    })
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
