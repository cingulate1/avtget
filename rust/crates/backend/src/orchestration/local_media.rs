use std::fs;
use std::path::{Path, PathBuf};

use avtget_config::resolve_relative_to_config;
use avtget_domain::{
    ArtifactKind, AutoCleanTranscript, BackendError, BackendEvent, ClipRange, JobConfig, Result,
    Settings,
};

use crate::adapters::{
    FfmpegAdapter, TranscriptCleanRequest, TranscriptCleanerAdapter, WhisperBridgeAdapter,
    WhisperBridgeRequest,
};
use crate::cancel::CancellationToken;
use crate::events::EventEmitter;

use super::routing::{effective_modes, LocalInputKind, LocalInputPlan};

pub struct LocalMediaExecutionRequest<'a> {
    pub config_path: &'a Path,
    pub job_config: &'a JobConfig,
    pub settings: &'a Settings,
    pub plans: &'a [LocalInputPlan],
    pub python_executable: &'a str,
    pub python_whisper_bridge: &'a Path,
    pub cleaner_bridge: Option<&'a Path>,
}

pub struct LocalMediaOrchestrator<'a, TFfmpeg, TWhisper, TCleaner>
where
    TFfmpeg: FfmpegAdapter,
    TWhisper: WhisperBridgeAdapter,
    TCleaner: TranscriptCleanerAdapter,
{
    emitter: EventEmitter,
    cancel_token: CancellationToken,
    ffmpeg: &'a TFfmpeg,
    whisper: &'a TWhisper,
    transcript_cleaner: &'a TCleaner,
}

impl<'a, TFfmpeg, TWhisper, TCleaner> LocalMediaOrchestrator<'a, TFfmpeg, TWhisper, TCleaner>
where
    TFfmpeg: FfmpegAdapter,
    TWhisper: WhisperBridgeAdapter,
    TCleaner: TranscriptCleanerAdapter,
{
    pub fn new(
        emitter: EventEmitter,
        cancel_token: CancellationToken,
        ffmpeg: &'a TFfmpeg,
        whisper: &'a TWhisper,
        transcript_cleaner: &'a TCleaner,
    ) -> Self {
        Self {
            emitter,
            cancel_token,
            ffmpeg,
            whisper,
            transcript_cleaner,
        }
    }

    pub fn run(&self, request: LocalMediaExecutionRequest<'_>) -> Result<()> {
        self.ensure_not_cancelled()?;

        let (video_enabled, audio_enabled, transcript_enabled) =
            effective_modes(request.job_config, request.settings);
        let modes = EffectiveModes {
            video: video_enabled,
            audio: audio_enabled,
            transcript: transcript_enabled,
        };
        let model = choose_model(request.job_config, request.settings);
        let gpu = request
            .job_config
            .gpu
            .as_ref()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "0".to_owned());
        let verbose = request
            .job_config
            .verbose
            .unwrap_or(request.settings.default_verbose);
        let auto_clean_selector = AutoCleanTranscript::parse(
            request
                .job_config
                .auto_clean_transcript
                .as_deref()
                .unwrap_or(&request.settings.auto_clean_transcript),
        );
        let stage = stage_description(&modes, &model, request.settings);

        let storage_root =
            resolve_relative_to_config(request.config_path, &request.settings.storage_directory);
        let temp_dir =
            resolve_relative_to_config(request.config_path, &request.settings.temp_directory);
        let directories = LocalDirectories::from_root(storage_root, temp_dir);
        directories.ensure()?;

        let ffmpeg_path = resolve_tool_path(request.config_path, &request.settings.ffmpeg_path);
        let whisperx_path = resolve_tool_path(request.config_path, &request.settings.whisperx_path);
        let total = request.plans.len() as i64;

        for (index, plan) in request.plans.iter().enumerate() {
            self.ensure_not_cancelled()?;

            let current = (index + 1) as i64;
            let progress = if total > 0 {
                (current as f64 / total as f64) * 100.0
            } else {
                100.0
            };
            self.emitter.emit(BackendEvent::StageCount {
                stage_name: stage.clone(),
                current,
                total,
            })?;
            self.emitter.emit(BackendEvent::Progress {
                percent: progress,
                stage: stage.clone(),
            })?;
            self.emitter.emit(BackendEvent::StatusChange {
                item_id: plan.input_id.clone(),
                status: avtget_domain::JobStatus::Running,
            })?;

            let filestem = sanitize_filename(
                plan.path
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .unwrap_or("unknown_input"),
            );
            self.emitter.emit(BackendEvent::Log {
                message: local_input_type_message(plan.kind).to_owned(),
            })?;

            let item_result = match plan.kind {
                LocalInputKind::Audio => {
                    // Keep legacy local-audio sequencing exactly, including duplicate statuses.
                    self.emit_artifact_status(&plan.input_id, ArtifactKind::Filestem, &filestem)?;
                    self.emit_artifact_status(&plan.input_id, ArtifactKind::Video, "skipped")?;
                    self.emit_artifact_status(&plan.input_id, ArtifactKind::Audio, "skipped")?;
                    self.emit_artifact_status(&plan.input_id, ArtifactKind::Transcript, "running")?;

                    let result = self.process_local_audio(
                        plan,
                        &filestem,
                        &modes,
                        &model,
                        &gpu,
                        verbose,
                        request.settings.default_clips_full_output,
                        &directories,
                        ffmpeg_path.as_deref(),
                        whisperx_path.clone(),
                        request.python_executable,
                        request.python_whisper_bridge,
                        request.job_config,
                        request.settings,
                        &auto_clean_selector,
                        request.cleaner_bridge,
                    );
                    if result.is_ok() {
                        self.emit_artifact_status(
                            &plan.input_id,
                            ArtifactKind::Transcript,
                            "completed",
                        )?;
                    }
                    result
                }
                LocalInputKind::Video => {
                    // Legacy local-video path emits filestem before and during media processing.
                    self.emit_artifact_status(&plan.input_id, ArtifactKind::Filestem, &filestem)?;
                    self.process_local_video(
                        plan,
                        &filestem,
                        &modes,
                        &model,
                        &gpu,
                        verbose,
                        request.settings.default_clips_full_output,
                        &directories,
                        ffmpeg_path.as_deref(),
                        whisperx_path.clone(),
                        request.python_executable,
                        request.python_whisper_bridge,
                        request.job_config,
                        request.settings,
                        &auto_clean_selector,
                        request.cleaner_bridge,
                    )
                }
            };

            match item_result {
                Ok(()) => {
                    self.emitter.emit(BackendEvent::StatusChange {
                        item_id: plan.input_id.clone(),
                        status: avtget_domain::JobStatus::Completed,
                    })?;
                }
                Err(BackendError::Cancelled) => {
                    if plan.kind == LocalInputKind::Audio {
                        self.emit_artifact_status(
                            &plan.input_id,
                            ArtifactKind::Transcript,
                            "cancelled",
                        )?;
                    }
                    self.emitter.emit(BackendEvent::StatusChange {
                        item_id: plan.input_id.clone(),
                        status: avtget_domain::JobStatus::Cancelled,
                    })?;
                    self.emitter.emit_job_finished("Job cancelled by user")?;
                    return Err(BackendError::Cancelled);
                }
                Err(_error) => {
                    if plan.kind == LocalInputKind::Audio {
                        self.emit_artifact_status(
                            &plan.input_id,
                            ArtifactKind::Transcript,
                            "failed",
                        )?;
                    }
                    self.emitter.emit(BackendEvent::StatusChange {
                        item_id: plan.input_id.clone(),
                        status: avtget_domain::JobStatus::Failed,
                    })?;
                }
            }
        }

        self.emitter.emit_job_finished("Job completed")?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn process_local_audio(
        &self,
        plan: &LocalInputPlan,
        filestem: &str,
        modes: &EffectiveModes,
        model: &str,
        gpu: &str,
        verbose: bool,
        clips_full_output: bool,
        directories: &LocalDirectories,
        ffmpeg_path: Option<&str>,
        whisperx_path: Option<String>,
        python_executable: &str,
        python_whisper_bridge: &Path,
        job_config: &JobConfig,
        settings: &Settings,
        auto_clean_selector: &AutoCleanTranscript,
        cleaner_bridge: Option<&Path>,
    ) -> Result<()> {
        self.ensure_not_cancelled()?;
        self.emit_artifact_status(&plan.input_id, ArtifactKind::Filestem, filestem)?;
        let valid_clips = extract_valid_clips(&plan.clips);
        let has_clips = !valid_clips.is_empty();
        let audio_path = &plan.path;

        if modes.video {
            self.emit_artifact_status(&plan.input_id, ArtifactKind::Video, "skipped")?;
        }

        if modes.audio {
            if has_clips {
                self.emit_artifact_status(&plan.input_id, ArtifactKind::Audio, "running")?;
                let mut clips_created = 0usize;
                for (clip_index, clip) in valid_clips.iter().enumerate() {
                    self.ensure_not_cancelled()?;
                    let clip_output =
                        directories
                            .audio
                            .join(format!("{}_clip{}.mp3", filestem, clip_index + 1));
                    let created = self.ffmpeg.trim_media(
                        audio_path,
                        &clip_output,
                        clip.start,
                        clip.end,
                        ffmpeg_path,
                        verbose,
                    )?;
                    if created {
                        clips_created += 1;
                    }
                }
                if clips_created > 0 {
                    self.emit_artifact_status(&plan.input_id, ArtifactKind::Audio, "completed")?;
                } else {
                    self.emit_artifact_status(&plan.input_id, ArtifactKind::Audio, "failed")?;
                }
            } else {
                self.emit_artifact_status(&plan.input_id, ArtifactKind::Audio, "skipped")?;
            }
        }

        if modes.transcript {
            self.emitter.emit(BackendEvent::Progress {
                percent: 100.0,
                stage: transcript_stage_description(model),
            })?;
            self.emit_artifact_status(&plan.input_id, ArtifactKind::Transcript, "running")?;

            // Convert to 16 KHz mono WAV for optimal WhisperX input
            let whisper_wav = directories.temp.join(format!("{filestem}_whisper.wav"));
            if !whisper_wav.exists() {
                self.emitter.emit(BackendEvent::Log {
                    message: whisper_conversion_message(audio_path),
                })?;
                self.ffmpeg
                    .to_whisper_wav(audio_path, &whisper_wav, ffmpeg_path, verbose)?;
            }
            let transcription_source = if whisper_wav.exists() {
                whisper_wav.clone()
            } else {
                audio_path.to_path_buf()
            };

            let bridge_request = WhisperBridgeRequest {
                python_executable: python_executable.to_owned(),
                bridge_script: python_whisper_bridge.to_path_buf(),
                audio_path: transcription_source.clone(),
                output_dir: directories.transcripts.clone(),
                temp_dir: directories.temp.clone(),
                output_filestem: filestem.to_owned(),
                model: model.to_owned(),
                gpu: gpu.to_owned(),
                clips: plan.clips.clone(),
                clips_full_output,
                whisperx_path,
                ffmpeg_path: ffmpeg_path.map(str::to_owned),
            };

            let has_clips = !plan.clips.is_empty();
            let transcript_path = if has_clips {
                directories
                    .transcripts
                    .join(format!("{filestem}_clips.txt"))
            } else {
                directories.transcripts.join(format!("{filestem}.txt"))
            };
            match self.whisper.transcribe(bridge_request) {
                Ok(()) if transcript_path.exists() => {
                    self.run_auto_clean(
                        &transcript_path,
                        &plan.input_id,
                        directories,
                        settings,
                        auto_clean_selector,
                        cleaner_bridge,
                        python_executable,
                        verbose,
                    )?;
                    self.run_claude_clean_if_selected(
                        &transcript_path,
                        &plan.input_id,
                        filestem,
                        settings,
                        auto_clean_selector,
                    )?;
                    self.emit_artifact_status(
                        &plan.input_id,
                        ArtifactKind::Transcript,
                        "completed",
                    )?;
                    self.run_summarize_if_requested(
                        &transcript_path,
                        &plan.input_id,
                        filestem,
                        job_config,
                        settings,
                    )?;
                }
                Ok(()) => {
                    self.emitter.emit(BackendEvent::Log {
                        message: format!(
                            "WhisperX transcription failed: output not found at {}",
                            transcript_path.display()
                        ),
                    })?;
                    self.emit_artifact_status(&plan.input_id, ArtifactKind::Transcript, "failed")?
                }
                Err(err) => {
                    self.emitter.emit(BackendEvent::Log {
                        message: format!("WhisperX transcription failed: {}", err),
                    })?;
                    self.emit_artifact_status(&plan.input_id, ArtifactKind::Transcript, "failed")?
                }
            }
        }

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn process_local_video(
        &self,
        plan: &LocalInputPlan,
        filestem: &str,
        modes: &EffectiveModes,
        model: &str,
        gpu: &str,
        verbose: bool,
        clips_full_output: bool,
        directories: &LocalDirectories,
        ffmpeg_path: Option<&str>,
        whisperx_path: Option<String>,
        python_executable: &str,
        python_whisper_bridge: &Path,
        job_config: &JobConfig,
        settings: &Settings,
        auto_clean_selector: &AutoCleanTranscript,
        cleaner_bridge: Option<&Path>,
    ) -> Result<()> {
        self.ensure_not_cancelled()?;
        self.emit_artifact_status(&plan.input_id, ArtifactKind::Filestem, filestem)?;
        let video_path = &plan.path;
        let valid_clips = extract_valid_clips(&plan.clips);
        let has_clips = !valid_clips.is_empty();

        if modes.video {
            if has_clips {
                self.emit_artifact_status(&plan.input_id, ArtifactKind::Video, "running")?;
                let mut clips_created = 0usize;
                let extension = video_path
                    .extension()
                    .and_then(|value| value.to_str())
                    .unwrap_or("mp4");
                for (clip_index, clip) in valid_clips.iter().enumerate() {
                    self.ensure_not_cancelled()?;
                    let clip_output = directories.video.join(format!(
                        "{}_clip{}.{}",
                        filestem,
                        clip_index + 1,
                        extension
                    ));
                    let created = self.ffmpeg.trim_media(
                        video_path,
                        &clip_output,
                        clip.start,
                        clip.end,
                        ffmpeg_path,
                        verbose,
                    )?;
                    if created {
                        clips_created += 1;
                    }
                }
                if clips_full_output {
                    let full_output = directories
                        .video
                        .join(format!("{}.{}", filestem, extension));
                    if let Some(parent) = full_output.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    let _ = fs::copy(video_path, &full_output);
                }
                if clips_created > 0 {
                    self.emit_artifact_status(&plan.input_id, ArtifactKind::Video, "completed")?;
                } else {
                    self.emit_artifact_status(&plan.input_id, ArtifactKind::Video, "failed")?;
                }
            } else {
                self.emit_artifact_status(&plan.input_id, ArtifactKind::Video, "skipped")?;
            }
        }

        if modes.audio {
            self.emit_artifact_status(&plan.input_id, ArtifactKind::Audio, "running")?;
            if has_clips {
                let temp_audio = directories.temp.join(format!("{filestem}.mp3"));
                if !temp_audio.exists() {
                    self.emitter.emit(BackendEvent::Log {
                        message: audio_output_conversion_message(video_path),
                    })?;
                    self.ffmpeg
                        .video_to_audio(video_path, &temp_audio, ffmpeg_path, verbose)?;
                }
                let mut clips_created = 0usize;
                if temp_audio.exists() {
                    for (clip_index, clip) in valid_clips.iter().enumerate() {
                        self.ensure_not_cancelled()?;
                        let clip_output = directories.audio.join(format!(
                            "{}_clip{}.mp3",
                            filestem,
                            clip_index + 1
                        ));
                        let created = self.ffmpeg.trim_media(
                            &temp_audio,
                            &clip_output,
                            clip.start,
                            clip.end,
                            ffmpeg_path,
                            verbose,
                        )?;
                        if created {
                            clips_created += 1;
                        }
                    }
                }
                if clips_created > 0 {
                    if clips_full_output {
                        let full_output = directories.audio.join(format!("{filestem}.mp3"));
                        if let Some(parent) = full_output.parent() {
                            fs::create_dir_all(parent)?;
                        }
                        let _ = fs::copy(&temp_audio, &full_output);
                    }
                    self.emit_artifact_status(&plan.input_id, ArtifactKind::Audio, "completed")?;
                } else {
                    self.emit_artifact_status(&plan.input_id, ArtifactKind::Audio, "failed")?;
                }
            } else {
                let audio_output = directories.audio.join(format!("{filestem}.mp3"));
                if !audio_output.exists() {
                    self.emitter.emit(BackendEvent::Log {
                        message: audio_output_conversion_message(video_path),
                    })?;
                    self.ffmpeg
                        .video_to_audio(video_path, &audio_output, ffmpeg_path, verbose)?;
                }
                if audio_output.exists() {
                    self.emit_artifact_status(&plan.input_id, ArtifactKind::Audio, "completed")?;
                } else {
                    self.emit_artifact_status(&plan.input_id, ArtifactKind::Audio, "failed")?;
                }
            }
        }

        if modes.transcript {
            self.emitter.emit(BackendEvent::Progress {
                percent: 100.0,
                stage: transcript_stage_description(model),
            })?;
            self.emit_artifact_status(&plan.input_id, ArtifactKind::Transcript, "running")?;

            // Always extract a 16 KHz mono WAV for optimal WhisperX input
            let whisper_wav = directories.temp.join(format!("{filestem}_whisper.wav"));
            if !whisper_wav.exists() {
                self.emitter.emit(BackendEvent::Log {
                    message: whisper_conversion_message(video_path),
                })?;
                self.ffmpeg
                    .to_whisper_wav(video_path, &whisper_wav, ffmpeg_path, verbose)?;
            }
            let transcription_source = if whisper_wav.exists() {
                whisper_wav
            } else {
                // Fallback: extract MP3 to temp if WAV creation failed
                let temp_audio = directories.temp.join(format!("{filestem}.mp3"));
                if !temp_audio.exists() {
                    self.ffmpeg
                        .video_to_audio(video_path, &temp_audio, ffmpeg_path, verbose)?;
                }
                temp_audio
            };

            let bridge_request = WhisperBridgeRequest {
                python_executable: python_executable.to_owned(),
                bridge_script: python_whisper_bridge.to_path_buf(),
                audio_path: transcription_source,
                output_dir: directories.transcripts.clone(),
                temp_dir: directories.temp.clone(),
                output_filestem: filestem.to_owned(),
                model: model.to_owned(),
                gpu: gpu.to_owned(),
                clips: plan.clips.clone(),
                clips_full_output,
                whisperx_path,
                ffmpeg_path: ffmpeg_path.map(str::to_owned),
            };

            let has_clips = !plan.clips.is_empty();
            let transcript_path = if has_clips {
                directories
                    .transcripts
                    .join(format!("{filestem}_clips.txt"))
            } else {
                directories.transcripts.join(format!("{filestem}.txt"))
            };
            match self.whisper.transcribe(bridge_request) {
                Ok(()) if transcript_path.exists() => {
                    self.run_auto_clean(
                        &transcript_path,
                        &plan.input_id,
                        directories,
                        settings,
                        auto_clean_selector,
                        cleaner_bridge,
                        python_executable,
                        verbose,
                    )?;
                    self.run_claude_clean_if_selected(
                        &transcript_path,
                        &plan.input_id,
                        filestem,
                        settings,
                        auto_clean_selector,
                    )?;
                    self.emit_artifact_status(
                        &plan.input_id,
                        ArtifactKind::Transcript,
                        "completed",
                    )?;
                    self.run_summarize_if_requested(
                        &transcript_path,
                        &plan.input_id,
                        filestem,
                        job_config,
                        settings,
                    )?;
                }
                Ok(()) => {
                    self.emitter.emit(BackendEvent::Log {
                        message: format!(
                            "WhisperX transcription failed: output not found at {}",
                            transcript_path.display()
                        ),
                    })?;
                    self.emit_artifact_status(&plan.input_id, ArtifactKind::Transcript, "failed")?
                }
                Err(err) => {
                    self.emitter.emit(BackendEvent::Log {
                        message: format!("WhisperX transcription failed: {}", err),
                    })?;
                    self.emit_artifact_status(&plan.input_id, ArtifactKind::Transcript, "failed")?
                }
            }
        }

        Ok(())
    }

    /// Run Claude single-mode cleaning inline if selected — caller is expected
    /// to do this before emitting transcript=completed so the cleaned file is
    /// the canonical artifact.
    fn run_claude_clean_if_selected(
        &self,
        transcript_path: &Path,
        item_id: &str,
        filestem: &str,
        settings: &Settings,
        auto_clean_selector: &AutoCleanTranscript,
    ) -> Result<()> {
        if matches!(auto_clean_selector, AutoCleanTranscript::Claude) {
            let owned_paths = [transcript_path.to_path_buf()];
            let _ = crate::postprocess::clean_transcript_with_claude(
                &self.emitter,
                &self.cancel_token,
                item_id,
                filestem,
                &owned_paths,
                &settings.claude_model_effort,
            )?;
        }
        Ok(())
    }

    /// Run summarize on the canonical transcript if requested. The next plan
    /// item won't start until this returns, enforcing strict sequential
    /// processing across the batch.
    fn run_summarize_if_requested(
        &self,
        transcript_path: &Path,
        item_id: &str,
        filestem: &str,
        job_config: &JobConfig,
        settings: &Settings,
    ) -> Result<()> {
        if job_config.summarize {
            let summary_output = crate::postprocess::summary_output_path(transcript_path);
            crate::postprocess::summarize_transcript(
                &self.emitter,
                &self.cancel_token,
                settings,
                item_id,
                filestem,
                transcript_path,
                &summary_output,
            )?;
        }
        Ok(())
    }

    /// Run transcript auto-cleaning if enabled. Errors are logged but do not
    /// fail the overall job — a raw transcript is still better than nothing.
    #[allow(clippy::too_many_arguments)]
    fn run_auto_clean(
        &self,
        transcript_path: &Path,
        item_id: &str,
        directories: &LocalDirectories,
        settings: &Settings,
        selector: &AutoCleanTranscript,
        cleaner_bridge: Option<&Path>,
        python_executable: &str,
        _verbose: bool,
    ) -> Result<()> {
        let Some(cleaner) = cleaner_for_selector(selector) else {
            return Ok(());
        };
        if *selector == AutoCleanTranscript::Ollama
            && !crate::ollama::ensure_ollama_running(&self.emitter)
        {
            self.emitter.emit(BackendEvent::Log {
                message: format!("Transcript auto-clean skipped for {item_id}: Ollama not running"),
            })?;
            return Ok(());
        }
        let Some(bridge) = cleaner_bridge else {
            self.emitter.emit(BackendEvent::Log {
                message: format!(
                    "Transcript auto-clean skipped for {item_id}: cleaner bridge unavailable"
                ),
            })?;
            return Ok(());
        };
        if !transcript_path.exists() {
            return Ok(());
        }

        let stem = transcript_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("transcript");
        let clean_output = directories.temp.join(format!("{stem}_auto_clean.txt"));

        self.emitter.emit(BackendEvent::Log {
            message: format!("Starting transcript auto-clean for {item_id} (cleaner: {cleaner})"),
        })?;

        match self
            .transcript_cleaner
            .clean_transcript(TranscriptCleanRequest {
                python_executable: python_executable.to_owned(),
                bridge_script: bridge.to_path_buf(),
                transcript_path: transcript_path.to_path_buf(),
                output_path: clean_output.clone(),
                cleaner,
                log_file_path: Some(directories.temp.join("avtget_debug.log")),
            }) {
            Ok(outcome) if clean_output.exists() => {
                fs::copy(&clean_output, transcript_path)?;
                let provider = outcome.provider.as_deref().unwrap_or("unknown");
                let shards = outcome.shards_total.unwrap_or(1);
                let shard_note = if outcome.used_sharding.unwrap_or(shards > 1) {
                    format!("sharded ({shards} shards)")
                } else {
                    "single pass".to_owned()
                };
                self.emitter.emit(BackendEvent::Log {
                    message: format!(
                        "Transcript auto-clean completed for {item_id}: provider={provider}, {shard_note}"
                    ),
                })?;
            }
            Ok(_) => {
                self.emitter.emit(BackendEvent::Log {
                    message: format!(
                        "Transcript auto-clean failed for {item_id}: cleaner produced no output"
                    ),
                })?;
            }
            Err(err) => {
                self.emitter.emit(BackendEvent::Log {
                    message: format!("Transcript auto-clean failed for {item_id}: {err}"),
                })?;
            }
        }

        Ok(())
    }

    fn ensure_not_cancelled(&self) -> Result<()> {
        if self.cancel_token.is_cancelled() {
            return Err(BackendError::Cancelled);
        }
        Ok(())
    }

    fn emit_artifact_status(
        &self,
        item_id: &str,
        artifact: ArtifactKind,
        status: &str,
    ) -> Result<()> {
        self.emitter.emit(BackendEvent::ArtifactStatus {
            item_id: item_id.to_owned(),
            artifact,
            status: status.to_owned(),
        })
    }
}

struct LocalDirectories {
    video: PathBuf,
    audio: PathBuf,
    transcripts: PathBuf,
    temp: PathBuf,
}

impl LocalDirectories {
    fn from_root(storage_root: PathBuf, temp: PathBuf) -> Self {
        Self {
            video: storage_root.join("video"),
            audio: storage_root.join("audio"),
            transcripts: storage_root.join("transcripts"),
            temp,
        }
    }

    fn ensure(&self) -> Result<()> {
        fs::create_dir_all(&self.video)?;
        fs::create_dir_all(&self.audio)?;
        fs::create_dir_all(&self.transcripts)?;
        fs::create_dir_all(&self.temp)?;
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct EffectiveModes {
    video: bool,
    audio: bool,
    transcript: bool,
}

#[derive(Debug, Clone, Copy)]
struct ClipWindow {
    start: f64,
    end: f64,
}

fn choose_model(job_config: &JobConfig, settings: &Settings) -> String {
    if let Some(model) = job_config
        .model
        .as_ref()
        .map(|value| value.trim().to_owned())
    {
        if !model.is_empty() {
            return model;
        }
    }
    let configured = settings.default_model.trim();
    if !configured.is_empty() {
        return configured.to_owned();
    }
    "medium".to_owned()
}

fn stage_description(modes: &EffectiveModes, model: &str, settings: &Settings) -> String {
    if modes.video {
        return "video".to_owned();
    }
    if modes.audio {
        return "audio".to_owned();
    }
    if modes.transcript {
        if settings
            .default_transcript_source
            .eq_ignore_ascii_case("captions")
        {
            return "transcript -".to_owned();
        }
        return format!("transcript - whisperx {model} -");
    }
    "processing".to_owned()
}

fn transcript_stage_description(model: &str) -> String {
    format!("transcript - whisperx {model} -")
}

fn local_input_type_message(kind: LocalInputKind) -> &'static str {
    match kind {
        LocalInputKind::Audio => "Input type: local-media [audio]",
        LocalInputKind::Video => "Input type: local-media [video]",
    }
}

fn whisper_conversion_message(input_path: &Path) -> String {
    format!(
        "Converting {} to .wav for whisper transcription",
        extension_label(input_path)
    )
}

fn audio_output_conversion_message(input_path: &Path) -> String {
    format!(
        "Converting {} to .mp3 for audio output",
        extension_label(input_path)
    )
}

fn extension_label(path: &Path) -> String {
    path.extension()
        .and_then(|value| value.to_str())
        .map(|value| format!(".{}", value.to_ascii_lowercase()))
        .unwrap_or_else(|| "input".to_owned())
}

fn resolve_tool_path(config_path: &Path, configured: &str) -> Option<String> {
    let trimmed = configured.trim();
    if trimmed.is_empty() || trimmed == "." {
        return None;
    }
    let resolved = resolve_relative_to_config(config_path, trimmed);
    Some(resolved.to_string_lossy().into_owned())
}

fn cleaner_for_selector(selector: &AutoCleanTranscript) -> Option<String> {
    match selector {
        AutoCleanTranscript::Off => None,
        // Claude cleaning runs in-process, not via the Python bridge.
        // See orchestration/non_local.rs::cleaner_for_selector for details.
        AutoCleanTranscript::Claude => None,
        AutoCleanTranscript::Ollama => Some("ollama".to_owned()),
    }
}

fn sanitize_filename(raw: &str) -> String {
    let mut output = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if !ch.is_ascii() {
            continue;
        }
        match ch {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => {}
            _ => output.push(ch),
        }
    }
    // Strip trailing dots/spaces too: Windows silently removes them from
    // filenames, so keeping them would make our generated names diverge from
    // what actually lands on disk (kept in sync with non_local::sanitize_filename).
    let trimmed = output.trim().trim_end_matches(|c: char| c == '.' || c == ' ');
    if trimmed.is_empty() {
        "unknown_input".to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn extract_valid_clips(clips: &[ClipRange]) -> Vec<ClipWindow> {
    let mut windows = Vec::new();
    for clip in clips {
        if let (Some(start), Some(end)) = (parse_timestamp(&clip.start), parse_timestamp(&clip.end))
        {
            if end > start {
                windows.push(ClipWindow { start, end });
            }
        }
    }
    windows
}

fn parse_timestamp(raw: &str) -> Option<f64> {
    let value = raw.trim();
    if value.is_empty() {
        return None;
    }
    let parts: Vec<&str> = value.split(':').collect();
    match parts.as_slice() {
        [hours, minutes, seconds] => {
            let h: f64 = hours.parse().ok()?;
            let m: f64 = minutes.parse().ok()?;
            let s: f64 = seconds.parse().ok()?;
            Some((h * 3600.0) + (m * 60.0) + s)
        }
        [minutes, seconds] => {
            let m: f64 = minutes.parse().ok()?;
            let s: f64 = seconds.parse().ok()?;
            Some((m * 60.0) + s)
        }
        [seconds] => seconds.parse().ok(),
        _ => None,
    }
}
