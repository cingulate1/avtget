#!/usr/bin/env python3
"""
Video metadata bridge for the Rust backend.

This exposes legacy metadata extraction behavior from avtget_core:
- Selenium HTML scrape first (using geckodriver/firefox paths when provided)
- yt-dlp fallback when scrape is partial/unavailable
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Optional

SCRIPT_DIR = Path(__file__).parent
sys.path.insert(0, str(SCRIPT_DIR))

from avtget_core.avtget import VideoUtils  # noqa: E402


def _optional_path(raw: str) -> Optional[Path]:
    value = (raw or "").strip()
    if not value:
        return None
    return Path(value)


def main() -> int:
    parser = argparse.ArgumentParser(description="Video metadata bridge for Rust backend")
    parser.add_argument("--url", required=True)
    parser.add_argument("--browser", default="auto")
    parser.add_argument("--browser-path", default="")
    parser.add_argument("--verbose", action="store_true")
    args = parser.parse_args()

    try:
        title, channel = VideoUtils.get_metadata(
            args.url.strip(),
            verbose=bool(args.verbose),
            browser=args.browser.strip() or "auto",
            browser_path=_optional_path(args.browser_path),
        )
        print(
            json.dumps(
                {
                    "ok": True,
                    "title": title or "",
                    "channel": channel or "",
                }
            ),
            flush=True,
        )
        return 0
    except Exception as exc:
        print(json.dumps({"ok": False, "error": str(exc)}), flush=True)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
