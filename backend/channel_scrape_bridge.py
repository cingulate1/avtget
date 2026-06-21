#!/usr/bin/env python3
"""
Selenium channel scrape bridge for the Rust backend.

This script is intentionally narrow: it only exposes channel URL -> video URL
expansion using the existing Selenium scraper.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Optional

SCRIPT_DIR = Path(__file__).parent
sys.path.insert(0, str(SCRIPT_DIR))

from avtget_core.avtget import ChannelScraper  # noqa: E402


def _optional_path(raw: str) -> Optional[Path]:
    value = (raw or "").strip()
    if not value:
        return None
    return Path(value)


def main() -> int:
    parser = argparse.ArgumentParser(description="Channel scrape bridge for Rust backend")
    parser.add_argument("--channel-url", required=True)
    parser.add_argument("--timeframe-days", type=int, required=True)
    parser.add_argument("--browser", default="auto")
    parser.add_argument("--browser-path", default="")
    parser.add_argument("--verbose", action="store_true")
    args = parser.parse_args()

    try:
        urls = ChannelScraper.scrape_channel_videos(
            channel_url=args.channel_url.strip(),
            timeframe_days=max(int(args.timeframe_days), 0),
            browser=args.browser.strip() or "auto",
            browser_path=_optional_path(args.browser_path),
            verbose=bool(args.verbose),
        )
        print(
            json.dumps(
                {
                    "ok": True,
                    "urls": urls,
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
