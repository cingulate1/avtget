#!/usr/bin/env python3
"""
avtget_core.avtget - Utility module for bridge scripts.

Provides video/audio metadata extraction, channel scraping, and transcription
utilities consumed by the Rust backend via bridge scripts.
"""

import os
import sys
import json
import re
import subprocess
import threading
import warnings
from pathlib import Path
from datetime import datetime
from dataclasses import dataclass
from typing import Optional, Dict, List, Set, Tuple, Callable

CURRENT_DIR = Path(__file__).parent

# On Windows, suppress extra console windows for subprocesses when running the GUI/CLI.
RUN_OPTS = {"creationflags": subprocess.CREATE_NO_WINDOW} if os.name == "nt" else {}
LOG_FILE_PATH: Optional[Path] = None

from avtget_core.types import VideoSource

# -----------------------------------------------------------------------------
# Browser driver factory
# -----------------------------------------------------------------------------

def create_headless_driver(
    browser: str = "auto",
    browser_path: Optional[Path] = None,
    verbose: bool = False,
):
    """
    Create a headless Selenium WebDriver.

    Selenium Manager (built into selenium >= 4.6) handles downloading the
    correct driver binary automatically, so no manual chromedriver/geckodriver
    management is needed.

    Args:
        browser: "chrome", "firefox", or "auto" (tries Chrome first).
        browser_path: Optional path to the browser binary.
        verbose: Log which browser is being launched.
    """
    from selenium import webdriver

    def _try_chrome():
        from selenium.webdriver.chrome.options import Options as ChromeOptions
        opts = ChromeOptions()
        opts.add_argument("--headless=new")
        opts.add_argument("--disable-gpu")
        if browser_path and browser_path.exists():
            opts.binary_location = str(browser_path)
        if verbose:
            warnings.warn(f"Starting Chrome (headless){f' at {browser_path}' if browser_path else ''}")
        return webdriver.Chrome(options=opts)

    def _try_firefox():
        from selenium.webdriver.firefox.options import Options as FirefoxOptions
        opts = FirefoxOptions()
        opts.add_argument("--headless")
        if browser_path and browser_path.exists():
            opts.binary_location = str(browser_path)
        if verbose:
            warnings.warn(f"Starting Firefox (headless){f' at {browser_path}' if browser_path else ''}")
        return webdriver.Firefox(options=opts)

    browser = (browser or "auto").strip().lower()

    if browser == "chrome":
        return _try_chrome()
    elif browser == "firefox":
        return _try_firefox()
    else:
        # auto: try Chrome first (larger market share), fall back to Firefox
        try:
            return _try_chrome()
        except Exception as chrome_err:
            if verbose:
                warnings.warn(f"Chrome unavailable ({chrome_err}), trying Firefox...")
            return _try_firefox()


# -----------------------------------------------------------------------------
# Logging helpers
# -----------------------------------------------------------------------------

# Global set of active subprocess.Popen instances for cleanup on cancel
import threading
_ACTIVE_PROCESSES: Set[subprocess.Popen] = set()
_PROCESS_LOCK = threading.Lock()


class JobCancelledError(Exception):
    pass


def _kill_process_tree(pid: int) -> None:
    """
    Forcefully kill a process and all its children.
    Uses taskkill /F /T on Windows for aggressive termination (handles stubborn whisperx.exe).
    Falls back to psutil, then basic os.kill.
    """
    if os.name == 'nt':
        # Windows: Use taskkill with /F (force) and /T (tree kill) - most aggressive
        try:
            subprocess.run(
                ['taskkill', '/F', '/T', '/PID', str(pid)],
                capture_output=True,
                timeout=5,
                creationflags=subprocess.CREATE_NO_WINDOW
            )
            return
        except Exception:
            pass  # Fall through to psutil

    # Cross-platform: Try psutil
    try:
        import psutil
        parent = psutil.Process(pid)
        children = parent.children(recursive=True)
        # Kill children first (reverse order - deepest first)
        for child in reversed(children):
            try:
                child.kill()  # SIGKILL, not terminate
            except psutil.NoSuchProcess:
                pass
        # Then kill parent
        try:
            parent.kill()
        except psutil.NoSuchProcess:
            pass
        # Wait briefly for processes to die
        psutil.wait_procs([parent] + children, timeout=1)
    except ImportError:
        # psutil not available, fall back to basic termination
        try:
            import signal
            if os.name == 'nt':
                os.kill(pid, signal.SIGTERM)
            else:
                os.kill(pid, signal.SIGKILL)
        except (OSError, ProcessLookupError):
            pass
    except Exception:
        pass  # Best effort

def kill_all_active_processes() -> None:
    """Kill all tracked active processes. Called when cancel is requested."""
    with _PROCESS_LOCK:
        for proc in list(_ACTIVE_PROCESSES):
            try:
                if proc.poll() is None:  # Still running
                    _kill_process_tree(proc.pid)
            except Exception:
                pass
        _ACTIVE_PROCESSES.clear()

def set_run_log(temp_dir: Path) -> Path:
    """Initialize a run log file under the temp directory."""
    global LOG_FILE_PATH
    temp_dir.mkdir(parents=True, exist_ok=True)
    LOG_FILE_PATH = temp_dir / f"avtget_run_{datetime.now().strftime('%Y%m%d_%H%M%S')}.log"
    try:
        LOG_FILE_PATH.write_text(f"avtget run log started at {datetime.now().isoformat()}\n", encoding="utf-8")
    except Exception:
        pass
    return LOG_FILE_PATH


def _append_log(text: str) -> None:
    if not LOG_FILE_PATH:
        return
    try:
        with LOG_FILE_PATH.open("a", encoding="utf-8") as handle:
            handle.write(text + "\n")
    except Exception:
        pass


def _get_ytdlp_cmd() -> List[str]:
    """
    Get the correct yt-dlp command for the current environment.

    When running as a PyInstaller frozen bundle, sys.executable points to the .exe,
    not Python, so we need to use yt-dlp directly instead of 'python -m yt_dlp'.
    """
    import shutil

    # Check if we're running as a frozen PyInstaller bundle
    if getattr(sys, 'frozen', False):
        # Frozen: First try to find yt-dlp.exe in the same directory as the executable
        exe_dir = Path(sys.executable).parent
        local_ytdlp = exe_dir / 'yt-dlp.exe'
        if local_ytdlp.exists():
            return [str(local_ytdlp)]

        # Try to find yt-dlp.exe in PATH
        ytdlp_path = shutil.which('yt-dlp')
        if ytdlp_path:
            return [ytdlp_path]

        # Fallback: try yt-dlp without .exe (might work on some systems)
        return ['yt-dlp']
    else:
        # Development: Use module invocation
        return [sys.executable, '-m', 'yt_dlp']


def run_and_log(cmd: List[str], *, verbose: bool = False, text: bool = True, check: bool = True,
                stop_event: Optional[threading.Event] = None, **kwargs) -> subprocess.CompletedProcess:
    """
    Run a subprocess, writing full stdout/stderr to the temp log. Does not change UI verbosity.

    If stop_event is provided and set, terminates the subprocess and raises JobCancelledError.
    Uses Popen with polling to allow interruption instead of blocking subprocess.run.
    """
    import threading

    merged_kwargs = {**RUN_OPTS, **kwargs}
    capture = "stdout" not in merged_kwargs and "stderr" not in merged_kwargs
    if capture:
        merged_kwargs.setdefault("stdout", subprocess.PIPE)
        merged_kwargs.setdefault("stderr", subprocess.PIPE)
        merged_kwargs.setdefault("text", text)
        merged_kwargs.setdefault("bufsize", 1)  # Line buffered
    else:
        merged_kwargs.pop("capture_output", None)
    # Always redirect stdin to prevent blocking on parent's IPC channel
    merged_kwargs.setdefault("stdin", subprocess.DEVNULL)

    _append_log(f"[cmd] {' '.join(cmd)}")

    stdout_lines = []
    stderr_lines = []

    def log_stream(stream, lines_list, is_stderr):
        """Read stream line by line and log/store it."""
        try:
            for line in iter(stream.readline, ''):
                if not line:
                    break
                lines_list.append(line)
                clean_line = line.rstrip()

                # Suppress yt-dlp [download] progress lines in verbose output (can be very long)
                is_download_progress = clean_line.strip().startswith("[download]")

                # Suppress yt-dlp impersonation warnings (functionally irrelevant)
                is_impersonation_warning = (
                    "impersonation" in clean_line.lower() and
                    "impersonate target is available" in clean_line.lower()
                )

                if capture and not is_download_progress and not is_impersonation_warning:
                     prefix = "[stderr] " if is_stderr else "[stdout] "
                     _append_log(f"{prefix}{clean_line}")

                if verbose and capture and not is_download_progress and not is_impersonation_warning:
                    if is_stderr:
                        print(line, file=sys.stderr, end="")
                    else:
                        print(line, end="")
        except (ValueError, OSError):
            pass  # Handle stream closed
        finally:
            stream.close()

    try:
        proc = subprocess.Popen(cmd, **merged_kwargs)

        # Track the process globally for cleanup
        _ACTIVE_PROCESSES.add(proc)

        threads = []
        if capture:
            if proc.stdout:
                t_out = threading.Thread(target=log_stream, args=(proc.stdout, stdout_lines, False))
                t_out.daemon = True
                t_out.start()
                threads.append(t_out)
            if proc.stderr:
                t_err = threading.Thread(target=log_stream, args=(proc.stderr, stderr_lines, True))
                t_err.daemon = True
                t_err.start()
                threads.append(t_err)

        try:
            # Poll for completion while checking stop_event
            while proc.poll() is None:
                # Check for cancellation
                if stop_event and stop_event.is_set():
                    _append_log("[cancel] Terminating subprocess due to stop request")
                    _kill_process_tree(proc.pid)
                    raise JobCancelledError()

                # Small sleep to avoid busy-waiting
                import time
                time.sleep(0.1)

            # Wait for reader threads to finish to ensure we have all output
            for t in threads:
                t.join(timeout=1.0)

        finally:
            _ACTIVE_PROCESSES.discard(proc)

        # Reconstruct full output strings
        stdout_text = "".join(stdout_lines)
        stderr_text = "".join(stderr_lines)

        # Build CompletedProcess result
        result = subprocess.CompletedProcess(
            args=cmd,
            returncode=proc.returncode,
            stdout=stdout_text if capture else None,
            stderr=stderr_text if capture else None
        )

    except JobCancelledError:
        raise  # Re-raise cancellation
    except Exception as exc:
        _append_log(f"[error launching] {exc}")
        raise

    _append_log(f"[returncode] {result.returncode}")

    if check and result.returncode != 0:
        raise subprocess.CalledProcessError(result.returncode, cmd, output=result.stdout, stderr=result.stderr)
    return result


# =============================================================================
# PODCAST UTILITIES
# =============================================================================

class PodcastFetcher:
    """Handles podcast RSS feed fetching and parsing."""

    OVERCAST_PATTERN = r'overcast\.fm/itunes(\d+)'
    OVERCAST_EPISODE_PATTERN = r'overcast\.fm/\+([\w-]+)'
    ITUNES_LOOKUP_URL = 'https://itunes.apple.com/lookup?id={}'

    @staticmethod
    def is_overcast_url(url: str) -> bool:
        """Check if URL is an Overcast.fm iTunes-style link."""
        return bool(re.search(PodcastFetcher.OVERCAST_PATTERN, url))

    @staticmethod
    def is_overcast_episode_url(url: str) -> bool:
        """Check if URL is an Overcast single episode link."""
        return bool(re.search(PodcastFetcher.OVERCAST_EPISODE_PATTERN, url))

    @staticmethod
    def is_rss_url(url: str) -> bool:
        """Check if URL looks like an RSS feed."""
        url_lower = url.lower()
        if url_lower.endswith('.xml') or url_lower.endswith('.rss'):
            return True
        if any(p in url_lower for p in ['/feed/', '/rss/', '/podcast.xml']):
            return True
        return False


# =============================================================================
# VIDEO UTILITIES (YouTube + Twitch)
# =============================================================================

# Audio file extensions for local file detection
AUDIO_EXTENSIONS = {'.mp3', '.wav', '.flac', '.m4a', '.ogg', '.opus', '.wma', '.aac'}

class VideoUtils:
    """Utilities for video URL parsing and metadata extraction across platforms."""

    # YouTube patterns
    YOUTUBE_VIDEO_ID_PATTERNS = [
        r'(?:youtube\.com/watch\?v=|youtu\.be/)([a-zA-Z0-9_-]{11})',
        r'youtube\.com/embed/([a-zA-Z0-9_-]{11})',
    ]

    # Twitch VOD pattern: https://www.twitch.tv/videos/1234567890
    TWITCH_VOD_PATTERN = r'twitch\.tv/videos/(\d+)'

    @staticmethod
    def is_local_audio_file(path_or_url: str) -> bool:
        """Check if the input is a local audio file path."""
        if not path_or_url:
            return False
        # Check if it looks like a URL
        if path_or_url.startswith(('http://', 'https://', 'www.')):
            return False
        # Check if it's a path with audio extension
        path = Path(path_or_url)
        return path.suffix.lower() in AUDIO_EXTENSIONS and path.exists()

    @staticmethod
    def is_direct_audio_url(url: str) -> bool:
        """Check if URL is a direct link to an audio file."""
        if not url or not url.startswith(('http://', 'https://')):
            return False
        # Strip fragment (e.g., #t=0)
        url_no_fragment = url.split('#')[0]
        # Check if URL ends with audio extension
        url_lower = url_no_fragment.lower()
        return any(url_lower.endswith(ext) for ext in AUDIO_EXTENSIONS)

    @staticmethod
    def detect_source(url: str) -> VideoSource:
        """Detect the video source platform from URL."""
        # Check for local audio file first
        if VideoUtils.is_local_audio_file(url):
            return VideoSource.LOCAL_AUDIO
        url_lower = url.lower()
        if 'youtube.com' in url_lower or 'youtu.be' in url_lower:
            return VideoSource.YOUTUBE
        if 'twitch.tv' in url_lower:
            return VideoSource.TWITCH
        # Check for Overcast single episode
        if PodcastFetcher.is_overcast_episode_url(url):
            return VideoSource.OVERCAST
        # Check for podcast URLs
        if PodcastFetcher.is_overcast_url(url) or PodcastFetcher.is_rss_url(url):
            return VideoSource.PODCAST
        # Check for direct audio URL
        if VideoUtils.is_direct_audio_url(url):
            return VideoSource.DIRECT_AUDIO
        return VideoSource.UNKNOWN

    @staticmethod
    def extract_video_id(url: str) -> Optional[str]:
        """Extract a stable ID from a supported URL."""
        source = VideoUtils.detect_source(url)

        if source == VideoSource.YOUTUBE:
            url = url.split('&')[0]  # Remove query parameters
            for pattern in VideoUtils.YOUTUBE_VIDEO_ID_PATTERNS:
                if match := re.search(pattern, url):
                    return match.group(1)

        elif source == VideoSource.TWITCH:
            if match := re.search(VideoUtils.TWITCH_VOD_PATTERN, url):
                return f"twitch_{match.group(1)}"  # Prefix to avoid collision with YT IDs

        elif source == VideoSource.OVERCAST:
            if match := re.search(PodcastFetcher.OVERCAST_EPISODE_PATTERN, url):
                return f"overcast_{match.group(1)}"

        return None

    @staticmethod
    def clean_url(url: str) -> str:
        """Normalize video URL to standard format."""
        source = VideoUtils.detect_source(url)

        if source == VideoSource.YOUTUBE:
            # Extract and rebuild YouTube URL
            url_cleaned = url.split('&')[0]
            for pattern in VideoUtils.YOUTUBE_VIDEO_ID_PATTERNS:
                if match := re.search(pattern, url_cleaned):
                    return f'https://www.youtube.com/watch?v={match.group(1)}'

        elif source == VideoSource.TWITCH:
            # Normalize Twitch VOD URL
            if match := re.search(VideoUtils.TWITCH_VOD_PATTERN, url):
                return f'https://www.twitch.tv/videos/{match.group(1)}'

        return url

    @staticmethod
    def get_metadata_ytdlp(url: str, verbose: bool = False) -> Tuple[Optional[str], Optional[str]]:
        """Get video title and channel/uploader using yt-dlp (works for YouTube and Twitch)."""
        try:
            clean = VideoUtils.clean_url(url)
            cmd = [*_get_ytdlp_cmd(), '--no-playlist', '--dump-json', clean]
            result = run_and_log(cmd, verbose=verbose, text=True, check=True)
            data = json.loads(result.stdout)
            title = data.get('title')
            # yt-dlp uses 'uploader' for Twitch and 'channel' or 'uploader' for YouTube
            channel = data.get('uploader') or data.get('channel')

            if title and channel:
                return title, channel

            if verbose:
                warnings.warn("yt-dlp returned incomplete metadata (title/channel missing).")
            return None, None

        except (subprocess.CalledProcessError, json.JSONDecodeError, Exception) as e:
            if verbose:
                warnings.warn(f"yt-dlp metadata extraction failed: {e}")
            return None, None

    @staticmethod
    def get_metadata_html_scrape(
        url: str,
        verbose: bool = False,
        timeout: int = 20,
        browser: str = "auto",
        browser_path: Optional[Path] = None,
    ) -> Tuple[Optional[str], Optional[str]]:
        """
        Get video title and channel by scraping the YouTube page HTML using Selenium.
        This is more reliable than yt-dlp for basic metadata extraction when YouTube
        changes their API.

        Patterns used:
        - Title: <title>VIDEO TITLE - YouTube</title>
        - Channel: /@CHANNEL_NAME in URL, or alt="CHANNEL_NAME" in avatar
        """
        import html as html_module

        try:
            from selenium.webdriver.support.ui import WebDriverWait
            from selenium.webdriver.support import expected_conditions as EC
            from selenium.webdriver.common.by import By
        except ImportError:
            if verbose:
                warnings.warn("Selenium not available for HTML scraping")
            return None, None

        driver = None
        try:
            url = VideoUtils.clean_url(url)

            try:
                driver = create_headless_driver(browser=browser, browser_path=browser_path, verbose=verbose)
            except Exception as e:
                if verbose:
                    warnings.warn(f"Failed to start browser: {e}")
                return None, None

            driver.set_page_load_timeout(timeout)
            driver.get(url)

            WebDriverWait(driver, 10).until(
                EC.presence_of_element_located((By.TAG_NAME, "title"))
            )

            import time
            time.sleep(1)

            content = driver.page_source

            title = None
            channel = None

            title_match = re.search(r'<title>([^<]+)</title>', content)
            if title_match:
                raw_title = html_module.unescape(title_match.group(1))
                if raw_title.endswith(' - YouTube'):
                    title = raw_title[:-10].strip()
                else:
                    title = raw_title.strip()

            owner_section = re.search(r'<ytd-video-owner-renderer[^>]*>.*?</ytd-video-owner-renderer>', content, re.DOTALL)
            if owner_section:
                owner_html = owner_section.group(0)

                channel_name_match = re.search(r'<ytd-channel-name[^>]*>.*?<yt-formatted-string[^>]*id="text"[^>]*title="([^"]+)"', owner_html, re.DOTALL)
                if channel_name_match:
                    channel = html_module.unescape(channel_name_match.group(1))

                if not channel:
                    avatar_match = re.search(r'<yt-img-shadow[^>]*alt="([^"]+)"', owner_html)
                    if avatar_match:
                        channel = html_module.unescape(avatar_match.group(1))

                if not channel:
                    handle_match = re.search(r'/@([a-zA-Z0-9_-]+)"', owner_html)
                    if handle_match:
                        channel = handle_match.group(1)

            if title and channel:
                if verbose:
                    print(f"[HTML scrape] Title: {title}, Channel: {channel}")
                return title, channel

            if verbose:
                warnings.warn(f"HTML scrape incomplete: title={title}, channel={channel}")
            return title, channel

        except Exception as e:
            if verbose:
                warnings.warn(f"HTML scrape failed: {e}")
            return None, None
        finally:
            if driver:
                try:
                    driver.quit()
                except Exception:
                    pass

    @staticmethod
    def get_metadata(url: str, verbose: bool = False,
                     browser: str = "auto",
                     browser_path: Optional[Path] = None) -> Tuple[Optional[str], Optional[str]]:
        """
        Get video metadata (title, channel).

        Uses HTML scraping as the primary method (fast, doesn't hang),
        with yt-dlp as fallback for non-YouTube sources or if scraping fails.

        Args:
            url: Video URL
            verbose: Enable verbose logging
            browser: Browser to use (auto, chrome, firefox)
            browser_path: Optional browser binary path override
        """
        source = VideoUtils.detect_source(url)

        # For YouTube: try HTML scraping first (fast, reliable)
        if source == VideoSource.YOUTUBE:
            title, channel = VideoUtils.get_metadata_html_scrape(
                url,
                verbose,
                browser=browser,
                browser_path=browser_path,
            )
            if title and channel:
                return title, channel
            # If HTML scrape got partial results, keep them for fallback
            partial_title, partial_channel = title, channel
        else:
            partial_title, partial_channel = None, None

        # Fallback to yt-dlp (for Twitch, or if HTML scrape failed)
        title, channel = VideoUtils.get_metadata_ytdlp(url, verbose)

        # Merge partial results if needed
        if title and channel:
            return title, channel

        # Use any partial data we got
        return title or partial_title, channel or partial_channel


# =============================================================================
# MEDIA CONVERSION & TRANSCRIPTION
# =============================================================================

class MediaConverter:
    """Handles media conversion operations."""

    DEFAULT_WHISPERX_EXE = "whisperx"

    @staticmethod
    def _has_explicit_path(path_value: Optional[Path]) -> bool:
        if path_value is None:
            return False
        raw = str(path_value).strip()
        return raw not in {"", "."}

    @staticmethod
    def resolve_whisperx_exe(whisperx_path: Optional[Path] = None) -> str:
        """Resolve WhisperX executable in priority order: config path -> env var -> PATH."""
        if MediaConverter._has_explicit_path(whisperx_path):
            return str(whisperx_path)

        env_value = os.environ.get("WHISPERX_EXE", "").strip()
        if env_value:
            return env_value

        return MediaConverter.DEFAULT_WHISPERX_EXE

    @staticmethod
    def transcribe_audio(audio_path: Path, output_dir: Path, model: str,
                        gpu: str = '0', verbose: bool = False,
                        temp_dir: Optional[Path] = None,
                        output_filestem: Optional[str] = None,
                        whisperx_path: Optional[Path] = None,
                        ffmpeg_path: Optional[Path] = None):
        """Transcribe audio using WhisperX.

        WhisperX creates multiple output files (.json, .srt, .tsv, .txt, .vtt).
        This method outputs to temp_dir first, then moves only the .txt to output_dir.

        Args:
            audio_path: Path to audio file
            output_dir: Final destination for .txt transcript
            model: WhisperX model name
            gpu: CUDA device (physical GPU index)
            verbose: Enable verbose output
            temp_dir: Temporary directory for whisperx output (if None, uses output_dir directly)
            output_filestem: Desired filename stem for output (if None, uses audio_path.stem)
            whisperx_path: Optional explicit path/command for whisperx executable
            ffmpeg_path: Optional ffmpeg path used by whisperx subprocesses
        """
        import shutil

        # Use temp directory if provided, otherwise output directly
        whisperx_output_dir = temp_dir if temp_dir else output_dir
        whisperx_output_dir.mkdir(parents=True, exist_ok=True)

        # Build WhisperX command
        # WhisperX uses CUDA_VISIBLE_DEVICES env var + --device_index 0 for GPU selection
        whisperx_exe = MediaConverter.resolve_whisperx_exe(whisperx_path)
        cmd = [
            whisperx_exe, str(audio_path),
            '--model', model,
            '--device', 'cuda',
            '--device_index', '0',
            '--compute_type', 'float16',
            '--language', 'en',
            '--vad_method', 'silero',
            '--output_format', 'txt',
            '--output_dir', str(whisperx_output_dir),
        ]

        # Set CUDA_VISIBLE_DEVICES to select the physical GPU
        env = os.environ.copy()
        env['CUDA_VISIBLE_DEVICES'] = gpu

        # Add ffmpeg path for whisperx subprocess (whisperx calls ffmpeg internally)
        if MediaConverter._has_explicit_path(ffmpeg_path):
            ffmpeg_candidate = Path(str(ffmpeg_path))
            if ffmpeg_candidate.exists():
                ffmpeg_dir = ffmpeg_candidate if ffmpeg_candidate.is_dir() else ffmpeg_candidate.parent
                env["PATH"] = str(ffmpeg_dir) + os.pathsep + env.get("PATH", "")

        # IMPORTANT: Redirect ALL stdio (stdin/stdout/stderr) to prevent whisperx from
        # blocking on the parent's stdin (the Rust backend pipes JSON-lines IPC over it)
        _append_log(f"[cmd] {' '.join(cmd)}")
        _append_log(f"[env] CUDA_VISIBLE_DEVICES={gpu}")
        try:
            # Always redirect stdin to prevent any blocking on parent's IPC channel
            # Always capture stderr so we can surface error details on failure
            proc = subprocess.Popen(
                cmd,
                stdin=subprocess.DEVNULL,
                stdout=None if verbose else subprocess.DEVNULL,
                stderr=subprocess.PIPE,
                env=env
            )
            _, stderr_bytes = proc.communicate()
            returncode = proc.returncode
            _append_log(f"[returncode] {returncode}")
            if returncode != 0:
                stderr_text = (stderr_bytes or b"").decode("utf-8", errors="replace").strip()
                if stderr_text:
                    _append_log(f"[stderr] {stderr_text}")
                # Include last few lines of stderr in the error for diagnostics
                tail = "\n".join(stderr_text.splitlines()[-10:]) if stderr_text else ""
                msg = f"whisperx exited with code {returncode}"
                if tail:
                    msg += f":\n{tail}"
                raise RuntimeError(msg)
        except RuntimeError:
            raise
        except Exception as exc:
            _append_log(f"[error] {exc}")
            raise

        # Use provided output filestem or default to audio file stem
        audio_stem = audio_path.stem
        final_stem = output_filestem if output_filestem else audio_stem

        # If using temp directory, move .txt to final destination and clean up
        if temp_dir and temp_dir != output_dir:
            txt_file = temp_dir / f"{audio_stem}.txt"

            if txt_file.exists():
                output_dir.mkdir(parents=True, exist_ok=True)
                final_path = output_dir / f"{final_stem}.txt"
                # Use copy2 + unlink to avoid exclusive locking issues (move = copy+delete but more atomic locks)
                shutil.copy2(str(txt_file), str(final_path))
                # DELETED: We now keep temp files for data integrity

            # Clean up other whisperx output files
            # DELETED: We now keep temp files for data integrity
            # for ext in ['.json', '.srt', '.tsv', '.vtt']:
            #     temp_file = temp_dir / f"{audio_stem}{ext}"
            #     if temp_file.exists():
            #         try:
            #             temp_file.unlink()
            #         except OSError:
            #             pass
        elif output_filestem and output_filestem != audio_stem:
            # Not using temp, but need to rename the output file
            # Ideally this branch shouldn't be hit given our new "always temp" rule,
            # but patching it just in case.
            src_file = output_dir / f"{audio_stem}.txt"
            if src_file.exists():
                dst_file = output_dir / f"{final_stem}.txt"
                shutil.copy2(str(src_file), str(dst_file))
                # DELETED: We now keep temp files for data integrity


# =============================================================================
# CLIP PROCESSING
# =============================================================================

class ClipProcessor:
    """Handles clip extraction based on timestamp ranges."""

    @staticmethod
    def parse_timestamp(ts: str) -> Optional[float]:
        """Parse HH:MM:SS or MM:SS or SS format to seconds."""
        if not ts or not ts.strip():
            return None
        parts = ts.strip().split(':')
        try:
            if len(parts) == 3:
                h, m, s = parts
                return int(h) * 3600 + int(m) * 60 + float(s)
            elif len(parts) == 2:
                m, s = parts
                return int(m) * 60 + float(s)
            else:
                return float(parts[0])
        except (ValueError, IndexError):
            return None

    @staticmethod
    def transcribe_with_clips(audio_path: Path, output_dir: Path, model: str,
                              clips: List[Dict[str, str]], gpu: str = '0',
                              verbose: bool = False,
                              whisperx_path: Optional[Path] = None,
                              ffmpeg_path: Optional[Path] = None) -> Optional[Path]:
        """
        Transcribe audio with WhisperX and extract clip sections.
        Uses WhisperX's word-level timestamps when available.

        Returns path to the formatted clip transcript.
        """
        if not audio_path.exists():
            return None

        # First, run WhisperX with JSON output for timestamps
        # WhisperX uses CUDA_VISIBLE_DEVICES env var + --device_index 0 for GPU selection
        env = os.environ.copy()
        env['CUDA_VISIBLE_DEVICES'] = gpu

        if MediaConverter._has_explicit_path(ffmpeg_path):
            ffmpeg_candidate = Path(str(ffmpeg_path))
            if ffmpeg_candidate.exists():
                ffmpeg_dir = ffmpeg_candidate if ffmpeg_candidate.is_dir() else ffmpeg_candidate.parent
                env["PATH"] = str(ffmpeg_dir) + os.pathsep + env.get("PATH", "")

        # Get JSON output with word timestamps for accurate clip extraction
        json_output = output_dir / f"{audio_path.stem}.json"
        whisperx_exe = MediaConverter.resolve_whisperx_exe(whisperx_path)
        cmd = [
            whisperx_exe, str(audio_path),
            '--model', model,
            '--device', 'cuda',
            '--device_index', '0',
            '--compute_type', 'float16',
            '--language', 'en',
            '--output_format', 'json',
            '--output_dir', str(output_dir),
        ]

        try:
            proc = subprocess.Popen(
                cmd,
                stdin=subprocess.DEVNULL,
                stdout=None if verbose else subprocess.DEVNULL,
                stderr=None if verbose else subprocess.DEVNULL,
                env=env
            )
            returncode = proc.wait()
            if returncode != 0:
                raise subprocess.CalledProcessError(returncode, cmd)
        except subprocess.CalledProcessError as e:
            if verbose:
                warnings.warn(f"WhisperX transcription failed: {e}")
            return None

        if not json_output.exists():
            return None

        # Parse JSON output and extract clips
        try:
            with open(json_output, 'r', encoding='utf-8') as f:
                data = json.load(f)
        except (json.JSONDecodeError, Exception) as e:
            if verbose:
                warnings.warn(f"Failed to parse whisperx JSON output: {e}")
            return None

        segments = data.get('segments', [])

        # Build formatted output for each clip
        output_lines = []
        for i, clip in enumerate(clips, 1):
            start_str = clip.get('start', '')
            end_str = clip.get('end', '')
            if not start_str or not end_str:
                continue

            start_sec = ClipProcessor.parse_timestamp(start_str)
            end_sec = ClipProcessor.parse_timestamp(end_str)
            if start_sec is None or end_sec is None:
                continue

            output_lines.append(f"##clip{i}: {start_str} - {end_str}")

            # Extract segments within this time range
            clip_text = []
            for seg in segments:
                seg_start = seg.get('start', 0)
                seg_end = seg.get('end', 0)
                # Include segment if it overlaps with clip range
                if seg_end >= start_sec and seg_start <= end_sec:
                    text = seg.get('text', '').strip()
                    if text:
                        clip_text.append(text)

            if clip_text:
                output_lines.append(' '.join(clip_text))
            else:
                output_lines.append("[No content in this time range]")
            output_lines.append("")

        # Write clip transcript
        clip_output = output_dir / f"{audio_path.stem}_clips.txt"
        with open(clip_output, 'w', encoding='utf-8') as f:
            f.write('\n'.join(output_lines))

        # Also write plain text transcript
        txt_output = output_dir / f"{audio_path.stem}.txt"
        full_text = ' '.join(seg.get('text', '').strip() for seg in segments)
        with open(txt_output, 'w', encoding='utf-8') as f:
            f.write(full_text)

        return clip_output
