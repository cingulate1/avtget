#!/usr/bin/env python3
"""
Minimal Whisper bridge used by the Rust backend.

This script intentionally exposes only Whisper transcription behavior from the
Python stack. It does not run the full legacy job controller.
"""

from __future__ import annotations

import argparse
import json
import shutil
import sys
from pathlib import Path
from typing import Any, Dict, List, Optional

SCRIPT_DIR = Path(__file__).parent
sys.path.insert(0, str(SCRIPT_DIR))

from avtget_core.avtget import ClipProcessor, MediaConverter  # noqa: E402


def _emit_log(message: str) -> None:
    print(message, flush=True)


def _optional_path(raw: Optional[str]) -> Optional[Path]:
    if raw is None:
        return None
    value = raw.strip()
    if not value:
        return None
    return Path(value)


def _parse_clips(raw: Optional[str]) -> List[Dict[str, str]]:
    if raw is None or not raw.strip():
        return []
    payload = json.loads(raw)
    if not isinstance(payload, list):
        raise ValueError("clips_json must be a JSON array")
    clips: List[Dict[str, str]] = []
    for entry in payload:
        if not isinstance(entry, dict):
            continue
        start = str(entry.get("start", "")).strip()
        end = str(entry.get("end", "")).strip()
        if start and end:
            clips.append({"start": start, "end": end})
    return clips


def _copy_if_exists(source: Path, target: Path) -> bool:
    if not source.exists():
        return False
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source, target)
    return True


def _run(args: argparse.Namespace) -> Dict[str, Any]:
    audio_path = Path(args.audio_path)
    output_dir = Path(args.output_dir)
    temp_dir = Path(args.temp_dir)
    output_filestem = args.output_filestem.strip() if args.output_filestem else audio_path.stem
    model = args.model.strip()
    gpu = args.gpu.strip() if args.gpu else "0"
    whisperx_path = _optional_path(args.whisperx_path)
    ffmpeg_path = _optional_path(args.ffmpeg_path)
    clips = _parse_clips(args.clips_json)

    if not audio_path.exists():
        raise FileNotFoundError(f"Audio file not found: {audio_path}")
    if not model:
        raise ValueError("model is required")

    output_dir.mkdir(parents=True, exist_ok=True)
    temp_dir.mkdir(parents=True, exist_ok=True)

    transcript_path = output_dir / f"{output_filestem}.txt"
    clip_transcript_path = output_dir / f"{output_filestem}_clips.txt"

    _emit_log(f"Loaded WhisperX -- device: gpu {gpu}")
    _emit_log("Performing transcription")

    if clips:
        clips_full_output = getattr(args, 'clips_full_output', False)

        # Output to temp_dir first, then copy selected files to output_dir
        clip_output = ClipProcessor.transcribe_with_clips(
            audio_path,
            temp_dir,
            model,
            clips,
            gpu=gpu,
            verbose=False,
            whisperx_path=whisperx_path,
            ffmpeg_path=ffmpeg_path,
        )
        if clip_output is None:
            raise RuntimeError("Whisper clip transcription failed")

        source_stem = audio_path.stem
        raw_clips = temp_dir / f"{source_stem}_clips.txt"

        clips_ok = _copy_if_exists(raw_clips, clip_transcript_path)
        if not clips_ok:
            raise RuntimeError(f"Expected clip transcript not found: {raw_clips}")

        result: Dict[str, Any] = {
            "ok": True,
            "clip_transcript_path": str(clip_transcript_path),
        }

        if clips_full_output:
            raw_transcript = temp_dir / f"{source_stem}.txt"
            _copy_if_exists(raw_transcript, transcript_path)
            if transcript_path.exists():
                result["transcript_path"] = str(transcript_path)

        return result

    MediaConverter.transcribe_audio(
        audio_path=audio_path,
        output_dir=output_dir,
        model=model,
        gpu=gpu,
        verbose=False,
        temp_dir=temp_dir,
        output_filestem=output_filestem,
        whisperx_path=whisperx_path,
        ffmpeg_path=ffmpeg_path,
    )
    if not transcript_path.exists():
        raise RuntimeError(f"Expected transcript not found: {transcript_path}")

    return {
        "ok": True,
        "transcript_path": str(transcript_path),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description="Whisper bridge for Rust backend")
    parser.add_argument("--audio-path", required=True)
    parser.add_argument("--output-dir", required=True)
    parser.add_argument("--temp-dir", required=True)
    parser.add_argument("--output-filestem", required=True)
    parser.add_argument("--model", required=True)
    parser.add_argument("--gpu", default="0")
    parser.add_argument("--clips-json", default="")
    parser.add_argument("--clips-full-output", action="store_true")
    parser.add_argument("--whisperx-path", default="")
    parser.add_argument("--ffmpeg-path", default="")
    args = parser.parse_args()

    try:
        result = _run(args)
        print(json.dumps(result), flush=True)
        return 0
    except Exception as exc:
        payload = {"ok": False, "error": str(exc)}
        print(json.dumps(payload), flush=True)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
