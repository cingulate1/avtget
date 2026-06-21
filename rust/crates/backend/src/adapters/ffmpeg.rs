use std::path::Path;
use std::process::{Command, Stdio};

use avtget_domain::{BackendError, Result};

use super::FfmpegAdapter;

#[derive(Debug, Default)]
pub struct CliFfmpegAdapter;

impl CliFfmpegAdapter {
    fn resolve_ffmpeg_executable(ffmpeg_path: Option<&str>) -> String {
        match ffmpeg_path.map(str::trim) {
            Some(value) if !value.is_empty() && value != "." => value.to_owned(),
            _ => "ffmpeg".to_owned(),
        }
    }

    fn run_ffmpeg(mut command: Command) -> Result<bool> {
        command
            .arg("-hide_banner")
            .arg("-loglevel")
            .arg("error")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let status = command
            .status()
            .map_err(|err| BackendError::Process(format!("failed to launch ffmpeg: {err}")))?;
        Ok(status.success())
    }
}

impl FfmpegAdapter for CliFfmpegAdapter {
    fn trim_media(
        &self,
        input_path: &Path,
        output_path: &Path,
        start_seconds: f64,
        end_seconds: f64,
        ffmpeg_path: Option<&str>,
        verbose: bool,
    ) -> Result<bool> {
        if !input_path.exists() {
            return Ok(false);
        }

        let duration = end_seconds - start_seconds;
        if duration <= 0.0 {
            return Ok(false);
        }

        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let ffmpeg_exe = Self::resolve_ffmpeg_executable(ffmpeg_path);
        let mut command = Command::new(ffmpeg_exe);
        command
            .arg("-i")
            .arg(input_path)
            .arg("-ss")
            .arg(start_seconds.to_string())
            .arg("-t")
            .arg(duration.to_string())
            .arg("-c")
            .arg("copy")
            .arg(output_path)
            .arg("-y");

        Ok(Self::run_ffmpeg(command)? && output_path.exists())
    }

    fn video_to_audio(
        &self,
        video_path: &Path,
        audio_path: &Path,
        ffmpeg_path: Option<&str>,
        verbose: bool,
    ) -> Result<bool> {
        if !video_path.exists() {
            return Ok(false);
        }

        if let Some(parent) = audio_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let ffmpeg_exe = Self::resolve_ffmpeg_executable(ffmpeg_path);
        let mut command = Command::new(ffmpeg_exe);
        command
            .arg("-i")
            .arg(video_path)
            .arg("-vn")
            .arg("-ar")
            .arg("44100")
            .arg("-ac")
            .arg("2")
            .arg("-b:a")
            .arg("192k")
            .arg(audio_path)
            .arg("-y");

        Ok(Self::run_ffmpeg(command)? && audio_path.exists())
    }

    fn audio_to_mp3(
        &self,
        audio_path: &Path,
        output_path: &Path,
        ffmpeg_path: Option<&str>,
        verbose: bool,
    ) -> Result<bool> {
        if !audio_path.exists() {
            return Ok(false);
        }

        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let ffmpeg_exe = Self::resolve_ffmpeg_executable(ffmpeg_path);
        let mut command = Command::new(ffmpeg_exe);
        command
            .arg("-i")
            .arg(audio_path)
            .arg("-codec:a")
            .arg("libmp3lame")
            .arg("-qscale:a")
            .arg("2")
            .arg(output_path)
            .arg("-y");

        Ok(Self::run_ffmpeg(command)? && output_path.exists())
    }

    fn to_whisper_wav(
        &self,
        input_path: &Path,
        output_path: &Path,
        ffmpeg_path: Option<&str>,
        verbose: bool,
    ) -> Result<bool> {
        if !input_path.exists() {
            return Ok(false);
        }

        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let ffmpeg_exe = Self::resolve_ffmpeg_executable(ffmpeg_path);
        let mut command = Command::new(ffmpeg_exe);
        command
            .arg("-i")
            .arg(input_path)
            .arg("-vn")
            .arg("-ar")
            .arg("16000")
            .arg("-ac")
            .arg("1")
            .arg("-c:a")
            .arg("pcm_s16le")
            .arg(output_path)
            .arg("-y");

        Ok(Self::run_ffmpeg(command)? && output_path.exists())
    }
}
