use std::process::{Command, Output, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use avtget_domain::{BackendError, Result};

use super::{ChannelScrapeAdapter, ChannelScrapeRequest, YtDlpAdapter};

#[derive(Debug, Default)]
pub struct CliYtDlpAdapter {
    /// Browser name for --cookies-from-browser (e.g. "chrome", "firefox").
    /// Empty or "auto" means no cookies are passed.
    pub cookies_browser: String,
}

impl CliYtDlpAdapter {
    fn cookie_args(browser: &str) -> Vec<String> {
        if browser.is_empty() {
            return Vec::new();
        }
        vec!["--cookies-from-browser".to_owned(), browser.to_owned()]
    }

    fn effective_browser(&self) -> &str {
        let b = self.cookies_browser.trim();
        if b.is_empty() {
            return "";
        }
        if b == "auto" {
            "chrome"
        } else {
            b
        }
    }

    fn is_chromium_browser(browser: &str) -> bool {
        matches!(
            browser,
            "chrome" | "chromium" | "edge" | "brave" | "opera" | "vivaldi"
        )
    }

    fn output_contains(output: &Output, needle: &str) -> bool {
        String::from_utf8_lossy(&output.stderr).contains(needle)
            || String::from_utf8_lossy(&output.stdout).contains(needle)
    }

    fn run_ytdlp(
        python_executable: &str,
        args: &[String],
        extra_args: &[String],
        capture_output: bool,
    ) -> Result<Output> {
        let mut direct = Command::new("yt-dlp");
        direct.args(args).args(extra_args);
        if capture_output {
            direct.stdout(Stdio::piped()).stderr(Stdio::piped());
        }

        match direct.output() {
            Ok(output) => Ok(output),
            Err(first_err) => {
                if first_err.kind() != std::io::ErrorKind::NotFound {
                    return Err(BackendError::Process(format!(
                        "failed launching yt-dlp: {first_err}"
                    )));
                }

                let mut module = Command::new(python_executable);
                module.arg("-m").arg("yt_dlp").args(args).args(extra_args);
                if capture_output {
                    module.stdout(Stdio::piped()).stderr(Stdio::piped());
                }
                module.output().map_err(|err| {
                    BackendError::Process(format!(
                        "failed launching yt-dlp fallback via python module: {err}"
                    ))
                })
            }
        }
    }

    fn execute_with_fallback(
        &self,
        python_executable: &str,
        args: &[String],
        capture_output: bool,
    ) -> Result<Output> {
        let browser = self.effective_browser();
        let output = Self::run_ytdlp(
            python_executable,
            args,
            &Self::cookie_args(browser),
            capture_output,
        )?;

        // Chromium browsers lock their cookie DB while open; fall back to Firefox
        if !output.status.success()
            && Self::is_chromium_browser(browser)
            && Self::output_contains(&output, "Could not copy Chrome cookie database")
        {
            eprintln!(
                "Chrome cookie extraction failed (Chromium browsers lock their cookie \
                 database while running). Falling back to Firefox cookies..."
            );
            let fallback = Self::run_ytdlp(
                python_executable,
                args,
                &Self::cookie_args("firefox"),
                capture_output,
            )?;
            if !fallback.status.success() && Self::output_contains(&fallback, "Sign in to confirm")
            {
                Self::emit_auth_guidance();
            }
            return Ok(fallback);
        }

        if !output.status.success() && Self::output_contains(&output, "Sign in to confirm") {
            Self::emit_auth_guidance();
        }

        Ok(output)
    }

    fn emit_auth_guidance() {
        eprintln!(
            "YouTube bot detection: yt-dlp could not authenticate with YouTube. \
             To fix this, set the browser to 'firefox' in avtget Settings and log \
             into YouTube in Firefox. Firefox is the only browser that allows \
             external cookie access while the browser is open."
        );
    }

    fn common_args(verbose: bool) -> Vec<String> {
        let mut args = vec![
            // YouTube requires a JS runtime (deno) to solve n-parameter challenges;
            // allow yt-dlp to download the solver script from GitHub.
            "--remote-components".to_owned(),
            "ejs:github".to_owned(),
        ];
        if !verbose {
            args.push("--no-warnings".to_owned());
            args.push("--quiet".to_owned());
        }
        args
    }

    fn should_prefer_ffmpeg_hls(url: &str, format_selector: &str) -> bool {
        let lower_url = url.to_ascii_lowercase();
        let lower_format = format_selector.to_ascii_lowercase();
        lower_url.contains("twitch.tv/") && lower_format.contains("bestvideo")
    }

    fn ensure_success(output: Output, command_name: &str) -> Result<Output> {
        if output.status.success() {
            return Ok(output);
        }
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        let detail = if !stderr.is_empty() {
            stderr
        } else if !stdout.is_empty() {
            stdout
        } else {
            "unknown error".to_owned()
        };
        Err(BackendError::Process(format!(
            "{command_name} exited with code {:?}: {detail}",
            output.status.code()
        )))
    }
}

impl YtDlpAdapter for CliYtDlpAdapter {
    fn download_media(
        &self,
        python_executable: &str,
        url: &str,
        format_selector: &str,
        output_template: &str,
        ffmpeg_path: Option<&str>,
        verbose: bool,
    ) -> Result<()> {
        let mut args = vec![
            "--no-playlist".to_owned(),
            "-f".to_owned(),
            format_selector.to_owned(),
            "-o".to_owned(),
            output_template.to_owned(),
        ];
        if Self::should_prefer_ffmpeg_hls(url, format_selector) {
            args.push("--hls-prefer-ffmpeg".to_owned());
        }
        if let Some(path) = ffmpeg_path {
            let trimmed = path.trim();
            if !trimmed.is_empty() && trimmed != "." {
                args.push("--ffmpeg-location".to_owned());
                args.push(trimmed.to_owned());
            }
        }
        args.extend(Self::common_args(verbose));
        args.push(url.to_owned());
        let output = self.execute_with_fallback(python_executable, &args, true)?;
        Self::ensure_success(output, "yt-dlp download")?;
        Ok(())
    }

    fn download_subtitles(
        &self,
        python_executable: &str,
        url: &str,
        output_template: &str,
        ffmpeg_path: Option<&str>,
        verbose: bool,
    ) -> Result<()> {
        let mut args = vec![
            "--no-playlist".to_owned(),
            "--write-auto-subs".to_owned(),
            "--write-subs".to_owned(),
            "--skip-download".to_owned(),
            "--sub-lang".to_owned(),
            "en.*,en".to_owned(),
            "--sub-format".to_owned(),
            "vtt/srt".to_owned(),
            "-o".to_owned(),
            output_template.to_owned(),
            url.to_owned(),
        ];
        if let Some(path) = ffmpeg_path {
            let trimmed = path.trim();
            if !trimmed.is_empty() && trimmed != "." {
                args.push("--ffmpeg-location".to_owned());
                args.push(trimmed.to_owned());
            }
        }
        args.extend(Self::common_args(verbose));

        let output = self.execute_with_fallback(python_executable, &args, true)?;
        // Keep behavior tolerant: subtitle calls can fail yet still leave caption files.
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        if stderr.to_ascii_lowercase().contains("429")
            || stdout.to_ascii_lowercase().contains("429")
            || stderr.to_ascii_lowercase().contains("subtitle")
        {
            return Ok(());
        }
        Self::ensure_success(output, "yt-dlp subtitle download")?;
        Ok(())
    }
}

impl ChannelScrapeAdapter for CliYtDlpAdapter {
    /// Enumerate a channel's video URLs within the timeframe window using yt-dlp.
    ///
    /// Replaces the former Selenium DOM scrape: yt-dlp is already this project's
    /// download workhorse, is community-maintained against YouTube's frontend
    /// churn, and yields exact `upload_date`s. The channel's "Videos" tab is
    /// newest-first, so the far edge uses `--break-match-filters
    /// "upload_date>=FAR_CUTOFF"` to collect every in-range video and stop the
    /// walk at the first too-old one; `--lazy-playlist` makes that early stop
    /// actually save work.
    ///
    /// When the near edge ("from") isn't today, an additional `--match-filters
    /// "upload_date<=NEAR_CUTOFF"` skips the still-too-recent uploads at the top
    /// of the tab WITHOUT breaking the walk — so collection resumes once the
    /// dates fall into the window. from = today (`from_days == 0`) drops this
    /// filter entirely, reducing to the original single-window behavior.
    fn scrape_channel_urls(&self, request: ChannelScrapeRequest) -> Result<Vec<String>> {
        let videos_url = normalize_channel_videos_url(&request.channel_url);
        let far_cutoff = cutoff_yyyymmdd(request.to_days);

        let mut args = vec![
            "--lazy-playlist".to_owned(),
            "--break-match-filters".to_owned(),
            format!("upload_date>={far_cutoff}"),
            "--print".to_owned(),
            "%(webpage_url)s".to_owned(),
            // A single premiere/members-only entry shouldn't abort the walk
            // before we reach the older videos behind it.
            "--ignore-no-formats-error".to_owned(),
        ];
        // Near edge: skip (but don't break on) uploads newer than the window.
        if request.from_days > 0 {
            let near_cutoff = cutoff_yyyymmdd(request.from_days);
            args.push("--match-filters".to_owned());
            args.push(format!("upload_date<={near_cutoff}"));
        }
        if !request.verbose {
            args.push("--no-warnings".to_owned());
        }
        args.push(videos_url);

        let output = self.execute_with_fallback(&request.python_executable, &args, true)?;
        let code = output.status.code();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let urls: Vec<String> = stdout
            .lines()
            .map(str::trim)
            .filter(|line| line.starts_with("http"))
            .map(str::to_owned)
            .collect();

        // yt-dlp exits 101 when `--break-match-filters` stops the walk at the
        // first too-old video — the normal "reached the timeframe boundary"
        // outcome. Exit 0 means the whole channel was within range. Anything
        // else with no URLs collected is a genuine failure worth surfacing.
        let reached_boundary = matches!(code, Some(0) | Some(101));
        if !reached_boundary && urls.is_empty() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(BackendError::Process(format!(
                "yt-dlp channel enumeration failed (code {code:?}): {}",
                stderr.trim()
            )));
        }
        Ok(urls)
    }
}

/// Normalize a channel URL to its "Videos" tab so enumeration is scoped to
/// long-form uploads (mirrors the previous scraper's behavior).
fn normalize_channel_videos_url(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/');
    if trimmed.to_ascii_lowercase().contains("/videos") {
        trimmed.to_owned()
    } else {
        format!("{trimmed}/videos")
    }
}

/// Compute `today - days` as a `YYYYMMDD` string (UTC) for yt-dlp's
/// `upload_date` filter. The bound is inclusive, matching the prior
/// `age_in_days <= timeframe` semantics.
fn cutoff_yyyymmdd(days: i64) -> String {
    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days_since_epoch = now_secs / 86_400 - days.max(0);
    let (y, m, d) = civil_from_days(days_since_epoch);
    format!("{y:04}{m:02}{d:02}")
}

/// Convert days since the Unix epoch (1970-01-01) into a `(year, month, day)`
/// calendar date. Howard Hinnant's public-domain `civil_from_days` algorithm —
/// avoids pulling in a date crate for this single conversion.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = (mp + if mp < 10 { 3 } else { -9 }) as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::{civil_from_days, normalize_channel_videos_url};

    #[test]
    fn civil_from_days_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(10_957), (2000, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        // 2024 is a leap year — day after Feb 28 is Feb 29.
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
    }

    #[test]
    fn normalize_appends_videos_tab_once() {
        assert_eq!(
            normalize_channel_videos_url("https://www.youtube.com/@t3dotgg"),
            "https://www.youtube.com/@t3dotgg/videos"
        );
        assert_eq!(
            normalize_channel_videos_url("https://www.youtube.com/@t3dotgg/videos"),
            "https://www.youtube.com/@t3dotgg/videos"
        );
        assert_eq!(
            normalize_channel_videos_url("https://www.youtube.com/@t3dotgg/videos/"),
            "https://www.youtube.com/@t3dotgg/videos"
        );
        assert_eq!(
            normalize_channel_videos_url("https://www.youtube.com/channel/UC123"),
            "https://www.youtube.com/channel/UC123/videos"
        );
    }
}
