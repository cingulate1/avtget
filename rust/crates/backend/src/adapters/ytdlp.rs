use std::process::{Command, Output, Stdio};

use avtget_domain::{BackendError, Result};

use super::YtDlpAdapter;

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
