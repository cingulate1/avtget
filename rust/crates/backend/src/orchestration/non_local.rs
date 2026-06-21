use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use avtget_config::{load_settings, resolve_relative_to_config};
use avtget_domain::{
    ArtifactKind, AutoCleanTranscript, BackendError, BackendEvent, ClipRange, JobConfig, JobStatus,
    Result, Settings,
};

use crate::adapters::{
    AdapterOutcome, AdapterRequest, ChannelScrapeAdapter, ChannelScrapeRequest, FfmpegAdapter,
    TranscriptCleanOutcome, TranscriptCleanRequest, TranscriptCleanerAdapter, VideoMetadataAdapter,
    VideoMetadataRequest, WhisperBridgeAdapter, WhisperBridgeRequest, YtDlpAdapter,
};
use crate::cancel::CancellationToken;
use crate::events::EventEmitter;

use super::routing::{effective_modes, NonLocalReason};

const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "wav", "m4a", "flac", "aac", "ogg", "opus", "wma", "aiff", "alac",
];
pub struct NonLocalExecutionRequest {
    pub reason: NonLocalReason,
    pub adapter_request: AdapterRequest,
}

#[derive(Debug, Clone)]
pub struct NonLocalOrchestrator<'a, TYtDlp, TFfmpeg, TWhisper, TChannel, TCleaner, TMetadata>
where
    TYtDlp: YtDlpAdapter,
    TFfmpeg: FfmpegAdapter,
    TWhisper: WhisperBridgeAdapter,
    TChannel: ChannelScrapeAdapter,
    TCleaner: TranscriptCleanerAdapter,
    TMetadata: VideoMetadataAdapter,
{
    ytdlp: &'a TYtDlp,
    ffmpeg: &'a TFfmpeg,
    whisper: &'a TWhisper,
    channel_scrape: &'a TChannel,
    transcript_cleaner: &'a TCleaner,
    video_metadata: &'a TMetadata,
}

impl<'a, TYtDlp, TFfmpeg, TWhisper, TChannel, TCleaner, TMetadata>
    NonLocalOrchestrator<'a, TYtDlp, TFfmpeg, TWhisper, TChannel, TCleaner, TMetadata>
where
    TYtDlp: YtDlpAdapter,
    TFfmpeg: FfmpegAdapter,
    TWhisper: WhisperBridgeAdapter,
    TChannel: ChannelScrapeAdapter,
    TCleaner: TranscriptCleanerAdapter,
    TMetadata: VideoMetadataAdapter,
{
    pub fn new(
        ytdlp: &'a TYtDlp,
        ffmpeg: &'a TFfmpeg,
        whisper: &'a TWhisper,
        channel_scrape: &'a TChannel,
        transcript_cleaner: &'a TCleaner,
        video_metadata: &'a TMetadata,
    ) -> Self {
        Self {
            ytdlp,
            ffmpeg,
            whisper,
            channel_scrape,
            transcript_cleaner,
            video_metadata,
        }
    }

    pub fn run(&self, request: NonLocalExecutionRequest) -> Result<AdapterOutcome> {
        let adapter_request = request.adapter_request;
        let job_config: JobConfig = serde_json::from_str(&adapter_request.job_config_json)
            .map_err(|err| BackendError::InvalidJobConfig(err.to_string()))?;
        let settings = load_settings(&adapter_request.config_path)?;
        let emitter = adapter_request.emitter.clone();
        let cancel_token = adapter_request.cancel_token.clone();

        let outcome = self.execute_job(
            request.reason,
            &adapter_request,
            &job_config,
            &settings,
            &emitter,
            &cancel_token,
        );

        match outcome {
            Ok(summary) => {
                emitter.emit_job_finished(summary)?;
                Ok(AdapterOutcome {
                    exit_code: Some(0),
                    terminal_event_seen: true,
                })
            }
            Err(BackendError::Cancelled) => {
                emitter.emit_job_finished("Job cancelled by user")?;
                Ok(AdapterOutcome {
                    exit_code: Some(0),
                    terminal_event_seen: true,
                })
            }
            Err(err) => Err(err),
        }
    }

    fn execute_job(
        &self,
        reason: NonLocalReason,
        adapter_request: &AdapterRequest,
        job_config: &JobConfig,
        settings: &Settings,
        emitter: &EventEmitter,
        cancel_token: &CancellationToken,
    ) -> Result<String> {
        self.ensure_not_cancelled(cancel_token)?;

        let paths = RuntimePaths::resolve(&adapter_request.config_path, settings);
        paths.ensure_directories()?;

        let (video_enabled, audio_enabled, transcript_enabled) =
            effective_modes(job_config, settings);
        let modes = EffectiveModes {
            video: video_enabled,
            audio: audio_enabled,
            transcript: transcript_enabled,
        };
        let verbose = job_config.verbose.unwrap_or(settings.default_verbose);
        let model = choose_model(job_config, settings);
        let gpu = choose_gpu(job_config);
        let transcript_source = choose_transcript_source(job_config, settings);
        let auto_clean_selector = AutoCleanTranscript::parse(
            job_config
                .auto_clean_transcript
                .as_deref()
                .unwrap_or(&settings.auto_clean_transcript),
        );
        let ffmpeg_path = resolve_tool_path(&adapter_request.config_path, &settings.ffmpeg_path);
        let whisperx_path =
            resolve_tool_path(&adapter_request.config_path, &settings.whisperx_path);
        let browser = settings.browser.clone();
        let browser_path = resolve_tool_path(&adapter_request.config_path, &settings.browser_path);
        let cleaner_bridge = resolve_bridge_script(
            &adapter_request.config_path,
            adapter_request.python_whisper_bridge.as_deref(),
            "transcript_clean_bridge.py",
        );
        let metadata_bridge = resolve_bridge_script(
            &adapter_request.config_path,
            adapter_request.python_whisper_bridge.as_deref(),
            "video_metadata_bridge.py",
        );
        let channel_bridge = resolve_bridge_script(
            &adapter_request.config_path,
            adapter_request.python_whisper_bridge.as_deref(),
            "channel_scrape_bridge.py",
        );

        if !modes.video && !modes.audio && !modes.transcript {
            return Ok("No effective output modes selected".to_owned());
        }

        if job_config.refresh_index.unwrap_or(false) {
            emitter.emit(BackendEvent::Log {
                message: "refresh_index is not fully ported in Rust yet; skipping metadata refresh"
                    .to_owned(),
            })?;
            return Ok("Index refresh completed".to_owned());
        }

        let parsed = self.parse_inputs(
            job_config,
            settings,
            adapter_request,
            cancel_token,
            channel_bridge.as_deref(),
            verbose,
        )?;

        self.emit_unsupported_inputs(&parsed.unsupported_inputs, modes, emitter, cancel_token)?;

        if parsed.channel_url.is_some() && parsed.timeframe_days.is_some() {
            let timeframe_days = parsed.timeframe_days.unwrap_or(0);
            if parsed.urls.is_empty() {
                emitter.emit(BackendEvent::Log {
                    message: "No videos found in the specified timeframe".to_owned(),
                })?;
            } else {
                emitter.emit(BackendEvent::Log {
                    message: format!(
                        "Found {} video(s) from channel in the last {} days",
                        parsed.urls.len(),
                        timeframe_days
                    ),
                })?;
            }
        }

        let mut any_work = false;
        // Count items whose transcript was saved but whose requested cleaning
        // did not fully succeed, so the job summary can be honest rather than
        // unconditionally reporting "Job completed".
        let mut clean_warnings = 0usize;
        if !parsed.urls.is_empty() {
            any_work = true;
            clean_warnings += self.process_urls(
                &parsed.urls,
                job_config,
                settings,
                adapter_request,
                emitter,
                cancel_token,
                &paths,
                modes,
                &model,
                &gpu,
                &transcript_source,
                auto_clean_selector.clone(),
                ffmpeg_path.as_deref(),
                whisperx_path.clone(),
                browser.clone(),
                browser_path.clone(),
                metadata_bridge.as_deref(),
                verbose,
                cleaner_bridge.as_deref(),
            )?;
        }

        if !parsed.direct_audio_urls.is_empty() {
            any_work = true;
            let bridge = adapter_request
                .python_whisper_bridge
                .as_deref()
                .ok_or_else(|| {
                    BackendError::InvalidSettings(
                        "python whisper bridge path is required for direct audio transcript flow"
                            .to_owned(),
                    )
                })?;
            clean_warnings += self.process_direct_audio_urls(
                &parsed.direct_audio_urls,
                job_config,
                settings,
                adapter_request,
                emitter,
                cancel_token,
                &paths,
                modes,
                &model,
                &gpu,
                ffmpeg_path.as_deref(),
                whisperx_path.clone(),
                bridge,
                auto_clean_selector,
                verbose,
                cleaner_bridge.as_deref(),
            )?;
        }

        if !parsed.transcript_files.is_empty() {
            any_work = true;
            let bridge = cleaner_bridge.as_deref().ok_or_else(|| {
                BackendError::InvalidSettings(
                    "transcript_clean_bridge.py is required for transcript cleaning flow"
                        .to_owned(),
                )
            })?;
            clean_warnings += self.process_transcript_files(
                &parsed.transcript_files,
                job_config,
                settings,
                adapter_request,
                emitter,
                cancel_token,
                &paths,
                verbose,
                bridge,
            )?;
        }

        if !any_work {
            emitter.emit(BackendEvent::Log {
                message: format!(
                    "Non-local route selected ({reason:?}) but no runnable inputs found"
                ),
            })?;
            return Ok("No runnable inputs".to_owned());
        }

        if clean_warnings > 0 {
            Ok(format!(
                "Job completed with {clean_warnings} cleaning warning(s) — raw transcript(s) saved; see log"
            ))
        } else {
            Ok("Job completed".to_owned())
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn parse_inputs(
        &self,
        job_config: &JobConfig,
        settings: &Settings,
        adapter_request: &AdapterRequest,
        cancel_token: &CancellationToken,
        channel_bridge: Option<&Path>,
        verbose: bool,
    ) -> Result<ParsedInputs> {
        let mut parsed = ParsedInputs::default();

        let channel_url = job_config
            .inputs
            .iter()
            .map(|value| value.trim())
            .find(|value| is_channel_url(value))
            .map(str::to_owned);
        let timeframe_days = channel_url
            .as_ref()
            .and_then(|_| job_config.timeframe.as_ref())
            .and_then(|value| parse_timeframe_days(value));
        if let (Some(channel), Some(days)) = (channel_url.clone(), timeframe_days) {
            self.ensure_not_cancelled(cancel_token)?;
            let bridge = channel_bridge.ok_or_else(|| {
                BackendError::InvalidSettings(
                    "channel_scrape_bridge.py is required for channel timeframe scraping"
                        .to_owned(),
                )
            })?;
            adapter_request.emitter.emit(BackendEvent::Log {
                message: format!("Scraping channel URLs (last {} days)...", days),
            })?;
            let request = ChannelScrapeRequest {
                python_executable: adapter_request.python_executable.clone(),
                bridge_script: bridge.to_path_buf(),
                channel_url: channel.clone(),
                timeframe_days: days,
                browser: settings.browser.clone(),
                browser_path: resolve_tool_path(
                    &adapter_request.config_path,
                    &settings.browser_path,
                ),
                verbose,
            };
            let urls = self.channel_scrape.scrape_channel_urls(request)?;
            parsed.urls = urls
                .into_iter()
                .map(|url| UrlInput {
                    item_id: url.clone(),
                    url,
                    clips: Vec::new(),
                })
                .collect();
            parsed.channel_url = Some(channel);
            parsed.timeframe_days = Some(days);
            return Ok(parsed);
        }

        for (index, raw_input) in job_config.inputs.iter().enumerate() {
            self.ensure_not_cancelled(cancel_token)?;
            let input = raw_input.trim();
            if input.is_empty() {
                continue;
            }
            let path = PathBuf::from(input);
            if path.exists() && path.is_file() {
                if lowercase_extension(&path) == "txt" {
                    let content = fs::read_to_string(&path).unwrap_or_default();
                    let first_line = content
                        .lines()
                        .map(str::trim)
                        .find(|line| !line.is_empty())
                        .unwrap_or_default();
                    if looks_like_url(first_line) {
                        for line in content
                            .lines()
                            .map(str::trim)
                            .filter(|line| !line.is_empty())
                        {
                            push_url_like_input(
                                line,
                                clips_for_index(job_config, index),
                                &mut parsed,
                            );
                        }
                    } else {
                        parsed.transcript_files.push(path.clone());
                    }
                } else {
                    parsed.unsupported_inputs.push(input.to_owned());
                }
                continue;
            }
            if looks_like_url(input) {
                push_url_like_input(input, clips_for_index(job_config, index), &mut parsed);
            } else {
                parsed.unsupported_inputs.push(input.to_owned());
            }
        }

        parsed.channel_url = channel_url;
        parsed.timeframe_days = timeframe_days;
        Ok(parsed)
    }

    fn emit_unsupported_inputs(
        &self,
        inputs: &[String],
        modes: EffectiveModes,
        emitter: &EventEmitter,
        cancel_token: &CancellationToken,
    ) -> Result<()> {
        let mut seen = HashSet::new();
        for raw in inputs {
            self.ensure_not_cancelled(cancel_token)?;
            if raw.trim().is_empty() || !seen.insert(raw.clone()) {
                continue;
            }
            if looks_like_url(raw) {
                emitter.emit(BackendEvent::Log {
                    message: url_input_type_message(raw),
                })?;
            }
            let filestem = fallback_filestem_for_input(raw);
            emitter.emit(BackendEvent::Log {
                message: format!("Error: unsupported URL input: {raw}"),
            })?;
            self.emit_artifact_status(emitter, raw, ArtifactKind::Filestem, &filestem)?;
            if modes.video {
                self.emit_artifact_status(emitter, raw, ArtifactKind::Video, "failed")?;
            }
            if modes.audio {
                self.emit_artifact_status(emitter, raw, ArtifactKind::Audio, "failed")?;
            }
            if modes.transcript {
                self.emit_artifact_status(emitter, raw, ArtifactKind::Transcript, "failed")?;
            }
            emitter.emit(BackendEvent::StatusChange {
                item_id: raw.clone(),
                status: JobStatus::Failed,
            })?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn process_urls(
        &self,
        urls: &[UrlInput],
        job_config: &JobConfig,
        settings: &Settings,
        adapter_request: &AdapterRequest,
        emitter: &EventEmitter,
        cancel_token: &CancellationToken,
        paths: &RuntimePaths,
        modes: EffectiveModes,
        model: &str,
        gpu: &str,
        transcript_source: &TranscriptSource,
        auto_clean_selector: AutoCleanTranscript,
        ffmpeg_path: Option<&str>,
        whisperx_path: Option<String>,
        browser: String,
        browser_path: Option<String>,
        metadata_bridge: Option<&Path>,
        verbose: bool,
        cleaner_bridge: Option<&Path>,
    ) -> Result<usize> {
        let stage = stage_description(&modes, model, settings);
        let total = urls.len() as i64;
        let mut used_filestems: HashSet<String> = HashSet::new();
        // Number of items whose transcript was saved but whose requested Claude
        // cleaning did not fully succeed (surfaced as a `warning` artifact).
        let mut clean_warnings = 0usize;

        for (index_value, url_input) in urls.iter().enumerate() {
            self.ensure_not_cancelled(cancel_token)?;
            let current = (index_value + 1) as i64;
            let percent = if total > 0 {
                (current as f64 / total as f64) * 100.0
            } else {
                100.0
            };
            emitter.emit(BackendEvent::StageCount {
                stage_name: stage.clone(),
                current,
                total,
            })?;
            emitter.emit(BackendEvent::Progress {
                percent,
                stage: stage.clone(),
            })?;
            emitter.emit(BackendEvent::StatusChange {
                item_id: url_input.item_id.clone(),
                status: JobStatus::Running,
            })?;
            emitter.emit(BackendEvent::Log {
                message: url_input_type_message(&url_input.url),
            })?;

            let fallback_filestem = sanitize_filename(&fallback_filestem_for_input(&url_input.url));
            let mut filestem = fallback_filestem.clone();

            // Overcast episode URLs need special resolution: fetch the episode page
            // to extract the direct MP3 URL and metadata from HTML meta tags.
            let overcast_resolved = if is_overcast_episode_url(&url_input.url) {
                emitter.emit(BackendEvent::Log {
                    message: format!("Resolving Overcast episode {}...", url_input.url),
                })?;
                match resolve_overcast_episode(&url_input.url) {
                    Some(episode) => {
                        emitter.emit(BackendEvent::Log {
                            message: format!(
                                "Resolved Overcast episode: {}",
                                episode.title.as_deref().unwrap_or("(untitled)")
                            ),
                        })?;
                        // Use resolved metadata for the filename
                        let generated = generate_filename_from_template(
                            &settings.filename_template,
                            episode.channel.as_deref(),
                            episode.title.as_deref(),
                            index_value,
                            "overcast",
                        );
                        if !generated.trim().is_empty() {
                            filestem = generated;
                        }
                        Some(episode)
                    }
                    None => {
                        emitter.emit(BackendEvent::Log {
                            message: format!(
                                "Failed to resolve Overcast episode {} (will attempt yt-dlp fallback)",
                                url_input.url
                            ),
                        })?;
                        None
                    }
                }
            } else {
                None
            };

            // For non-Overcast URLs (or failed Overcast resolution), use the metadata bridge
            if overcast_resolved.is_none() {
                if let Some(bridge_script) = metadata_bridge {
                    emitter.emit(BackendEvent::Log {
                        message: format!("Fetching metadata for {}...", url_input.url),
                    })?;
                    let metadata_result =
                        self.video_metadata.fetch_metadata(VideoMetadataRequest {
                            python_executable: adapter_request.python_executable.clone(),
                            bridge_script: bridge_script.to_path_buf(),
                            url: url_input.url.clone(),
                            browser: browser.clone(),
                            browser_path: browser_path.clone(),
                            verbose,
                        });
                    match metadata_result {
                        Ok(metadata) if metadata.title.is_some() || metadata.channel.is_some() => {
                            let generated = generate_filename_from_template(
                                &settings.filename_template,
                                metadata.channel.as_deref(),
                                metadata.title.as_deref(),
                                index_value,
                                detect_source_for_url(&url_input.url),
                            );
                            if !generated.trim().is_empty() {
                                filestem = generated;
                            }
                        }
                        Ok(_) => {}
                        Err(err) => {
                            emitter.emit(BackendEvent::Log {
                                message: format!(
                                    "Metadata bridge failed for {}: {} (using fallback filename)",
                                    url_input.url, err
                                ),
                            })?;
                        }
                    }
                }
            }

            // Ensure unique filestems within the batch to prevent file collisions
            // in the temp directory (e.g. two videos both resolving to the same name).
            if !used_filestems.insert(filestem.clone()) {
                let base = filestem.clone();
                let mut counter = 2u32;
                loop {
                    filestem = format!("{base}_{counter}");
                    if used_filestems.insert(filestem.clone()) {
                        break;
                    }
                    counter += 1;
                }
            }

            self.emit_artifact_status(
                emitter,
                &url_input.item_id,
                ArtifactKind::Filestem,
                &filestem,
            )?;

            let mut video_ok = !modes.video;
            let mut audio_ok = !modes.audio;
            let mut transcript_ok = !modes.transcript;

            // When clips are set and full output is disabled, skip saving
            // full video/audio to the output directory.
            let has_clips = !url_input.clips.is_empty();
            let clips_skip_full = has_clips && !settings.default_clips_full_output;

            // Determine the effective download URL: for resolved Overcast episodes,
            // use the direct MP3 URL; otherwise use the original URL.
            let effective_download_url = overcast_resolved
                .as_ref()
                .map(|ep| ep.audio_url.as_str())
                .unwrap_or(&url_input.url);

            if modes.video {
                if clips_skip_full {
                    self.emit_artifact_status(
                        emitter,
                        &url_input.item_id,
                        ArtifactKind::Video,
                        "skipped",
                    )?;
                } else if overcast_resolved.is_some() {
                    emitter.emit(BackendEvent::Log {
                        message: "Skipping video download (Overcast episodes are audio-only)"
                            .to_owned(),
                    })?;
                    self.emit_artifact_status(
                        emitter,
                        &url_input.item_id,
                        ArtifactKind::Video,
                        "skipped",
                    )?;
                } else {
                    self.emit_artifact_status(
                        emitter,
                        &url_input.item_id,
                        ArtifactKind::Video,
                        "running",
                    )?;
                    emitter.emit(BackendEvent::Log {
                        message: format!("Downloading video for {}...", url_input.item_id),
                    })?;
                    let output_template = paths.temp_dir.join(format!("{filestem}.%(ext)s"));
                    let downloaded = self.ytdlp.download_media(
                        &adapter_request.python_executable,
                        &url_input.url,
                        "bestvideo+bestaudio/best",
                        &output_template.to_string_lossy(),
                        ffmpeg_path,
                        false, // suppress yt-dlp raw output; Rust-side step messages suffice
                    );
                    video_ok = downloaded.is_ok()
                        && self.copy_downloaded_video_to_storage(paths, &filestem, emitter)?;
                    emitter.emit(BackendEvent::Log {
                        message: format!(
                            "Video download {}",
                            if video_ok { "completed" } else { "failed" }
                        ),
                    })?;
                    self.emit_artifact_status(
                        emitter,
                        &url_input.item_id,
                        ArtifactKind::Video,
                        if video_ok { "completed" } else { "failed" },
                    )?;
                }
            }

            if modes.audio || modes.transcript {
                if modes.audio {
                    self.emit_artifact_status(
                        emitter,
                        &url_input.item_id,
                        ArtifactKind::Audio,
                        "running",
                    )?;
                }
                emitter.emit(BackendEvent::Log {
                    message: format!("Downloading audio for {}...", url_input.item_id),
                })?;
            }

            // For resolved Overcast episodes, download the MP3 directly with curl
            // instead of going through yt-dlp (which doesn't support overcast.fm).
            let source_audio = if overcast_resolved.is_some() {
                self.download_direct_audio(effective_download_url, &filestem, paths)?
            } else {
                self.download_audio_for_url(
                    &url_input.url,
                    &filestem,
                    &adapter_request.python_executable,
                    ffmpeg_path,
                    false, // suppress yt-dlp raw output
                    paths,
                )?
            };

            if modes.audio {
                if clips_skip_full {
                    self.emit_artifact_status(
                        emitter,
                        &url_input.item_id,
                        ArtifactKind::Audio,
                        "skipped",
                    )?;
                } else {
                    audio_ok = false;
                    if let Some(audio_path) = source_audio.as_ref() {
                        let target = paths.audio_dir.join(format!("{filestem}.mp3"));
                        audio_ok = if lowercase_extension(audio_path) == "mp3" {
                            emitter.emit(BackendEvent::Log {
                                message: format!("Copying audio to {}", target.display()),
                            })?;
                            if let Some(parent) = target.parent() {
                                fs::create_dir_all(parent)?;
                            }
                            fs::copy(audio_path, &target).is_ok() && target.exists()
                        } else {
                            emitter.emit(BackendEvent::Log {
                                message: format!("Converting audio to mp3: {}", target.display()),
                            })?;
                            self.ffmpeg
                                .audio_to_mp3(audio_path, &target, ffmpeg_path, verbose)
                                .unwrap_or(false)
                        };
                    }
                    self.emit_artifact_status(
                        emitter,
                        &url_input.item_id,
                        ArtifactKind::Audio,
                        if audio_ok { "completed" } else { "failed" },
                    )?;
                }
            }

            // Path of the canonical transcript that summarize should consume.
            // Set when the transcript pipeline produces a final file. For
            // "both" mode without Claude cleaning, summarize is skipped (no
            // canonical merged transcript exists).
            let mut summarize_source: Option<PathBuf> = None;
            let mut summarize_stem: String = filestem.clone();
            // Set when cleaning was requested (Claude) but did not fully succeed,
            // so the transcript is reported as a `warning` rather than a clean
            // `completed`, and the item is tallied into the job summary.
            let mut clean_warning = false;

            if modes.transcript {
                self.emit_artifact_status(
                    emitter,
                    &url_input.item_id,
                    ArtifactKind::Transcript,
                    "running",
                )?;
                let has_clips = !url_input.clips.is_empty();
                let clip_suffix = if has_clips { "_clips" } else { "" };

                if matches!(transcript_source, TranscriptSource::Both) {
                    // "Both" mode: run captions + whisper independently and write
                    // each to its own suffixed transcript file. When Claude
                    // cleaning is selected, the clean-transcript skill then
                    // reconciles them (or, if only one source was retrieved,
                    // cleans that one alone — see clean_both_mode). With cleaning
                    // off, the two files are left for manual reconciliation.
                    let yt_filestem = format!("{filestem}-yt");
                    let whisper_filestem = format!("{filestem}-whisper");
                    let yt_transcript_path = paths
                        .transcripts_dir
                        .join(format!("{yt_filestem}{clip_suffix}.txt"));
                    let whisper_transcript_path = paths
                        .transcripts_dir
                        .join(format!("{whisper_filestem}{clip_suffix}.txt"));

                    emitter.emit(BackendEvent::Log {
                        message: format!(
                            "Both mode: downloading captions for {}...",
                            url_input.item_id
                        ),
                    })?;
                    let yt_ok = self.download_captions_as_text(
                        &url_input.url,
                        &yt_filestem,
                        &yt_transcript_path,
                        &adapter_request.python_executable,
                        ffmpeg_path,
                        false, // suppress yt-dlp raw output
                        paths,
                    )?;
                    if !yt_ok {
                        emitter.emit(BackendEvent::Log {
                            message: format!(
                                "Captions unavailable for {} (whisper transcript will still run)",
                                url_input.item_id
                            ),
                        })?;
                    }

                    let whisper_ok = if let Some(audio_path) = source_audio.as_deref() {
                        self.transcribe_audio_path(
                            audio_path,
                            &whisper_filestem,
                            &whisper_transcript_path,
                            adapter_request,
                            model,
                            gpu,
                            ffmpeg_path,
                            whisperx_path.clone(),
                            url_input.clips.clone(),
                            settings.default_clips_full_output,
                            paths,
                            emitter,
                            verbose,
                        )?
                    } else {
                        emitter.emit(BackendEvent::Log {
                            message: format!(
                                "WhisperX skipped for {}: no audio available",
                                url_input.item_id
                            ),
                        })?;
                        false
                    };

                    transcript_ok = yt_ok || whisper_ok;
                    if transcript_ok {
                        emitter.emit(BackendEvent::Log {
                            message: format!(
                                "Both mode for {}: yt={}, whisper={}",
                                url_input.item_id,
                                if yt_ok { "ok" } else { "missing" },
                                if whisper_ok { "ok" } else { "missing" }
                            ),
                        })?;
                    }

                    // Claude cleaning reconciles the source(s) into a single
                    // canonical {filestem}.txt. When both transcripts exist they
                    // are merged; when only one was retrieved (e.g. yt-dlp can't
                    // pull captions from an Overcast/podcast link) we gracefully
                    // fall back to cleaning that single transcript rather than
                    // silently skipping cleaning altogether.
                    if transcript_ok && matches!(auto_clean_selector, AutoCleanTranscript::Claude) {
                        let outcome = self.clean_both_mode(
                            emitter,
                            cancel_token,
                            &url_input.item_id,
                            &filestem,
                            clip_suffix,
                            &yt_transcript_path,
                            yt_ok,
                            &whisper_transcript_path,
                            whisper_ok,
                            paths,
                            &settings.claude_model_effort,
                        )?;
                        summarize_source = outcome.summarize_source;
                        if !outcome.cleaned {
                            // Cleaning was requested but did not fully succeed —
                            // surface a warning instead of a clean "completed".
                            clean_warning = true;
                        }
                    } else if transcript_ok {
                        // Both mode without Claude cleaning: documented
                        // manual-reconciliation workflow. Leave the -yt/-whisper
                        // files in place; there is no canonical merged transcript,
                        // so summarize has no single source to consume.
                        emitter.emit(BackendEvent::Log {
                            message: format!(
                                "Both mode for {}: auto-clean is not Claude — keeping -yt/-whisper files for manual reconciliation (summary skipped)",
                                url_input.item_id
                            ),
                        })?;
                    }
                } else {
                    let transcript_path = paths
                        .transcripts_dir
                        .join(format!("{filestem}{clip_suffix}.txt"));
                    if matches!(transcript_source, TranscriptSource::Captions) {
                        emitter.emit(BackendEvent::Log {
                            message: format!("Downloading captions for {}...", url_input.item_id),
                        })?;
                    }
                    transcript_ok = match transcript_source {
                        TranscriptSource::Captions => self.download_captions_as_text(
                            &url_input.url,
                            &filestem,
                            &transcript_path,
                            &adapter_request.python_executable,
                            ffmpeg_path,
                            false, // suppress yt-dlp raw output
                            paths,
                        )?,
                        TranscriptSource::Whisper => false,
                        TranscriptSource::Both => unreachable!("handled above"),
                    };
                    if !transcript_ok {
                        if matches!(transcript_source, TranscriptSource::Captions) {
                            emitter.emit(BackendEvent::Log {
                                message: format!(
                                    "Captions not available for {}, falling back to WhisperX",
                                    url_input.item_id
                                ),
                            })?;
                        }
                        if let Some(audio_path) = source_audio.as_deref() {
                            transcript_ok = self.transcribe_audio_path(
                                audio_path,
                                &filestem,
                                &transcript_path,
                                adapter_request,
                                model,
                                gpu,
                                ffmpeg_path,
                                whisperx_path.clone(),
                                url_input.clips.clone(),
                                settings.default_clips_full_output,
                                paths,
                                emitter,
                                verbose,
                            )?;
                        } else {
                            emitter.emit(BackendEvent::Log {
                                message: format!(
                                    "Transcript failed for {}: no captions found and no audio available for WhisperX fallback",
                                    url_input.item_id
                                ),
                            })?;
                        }
                    }
                    if transcript_ok {
                        match self.maybe_auto_clean_transcript(
                            &transcript_path,
                            adapter_request,
                            settings,
                            paths,
                            auto_clean_selector.clone(),
                            cleaner_bridge,
                            verbose,
                            emitter,
                        ) {
                            Ok(Some(outcome)) => {
                                emitter.emit(BackendEvent::Log {
                                    message: format!(
                                        "Transcript auto-clean completed for {}",
                                        url_input.item_id
                                    ),
                                })?;
                                emit_transcript_cleaning_details(
                                    emitter,
                                    &url_input.item_id,
                                    &outcome,
                                )?;
                            }
                            Ok(None) => {
                                let reason = if cleaner_for_selector(&auto_clean_selector).is_none()
                                {
                                    "auto-clean disabled"
                                } else if cleaner_bridge.is_none() {
                                    "cleaner bridge unavailable"
                                } else {
                                    "not applicable"
                                };
                                emitter.emit(BackendEvent::Log {
                                    message: format!(
                                        "Transcript auto-clean skipped for {} ({})",
                                        url_input.item_id, reason
                                    ),
                                })?;
                            }
                            Err(err) => {
                                emitter.emit(BackendEvent::Log {
                                    message: format!(
                                        "Transcript auto-clean failed for {}: {}",
                                        url_input.item_id, err
                                    ),
                                })?;
                            }
                        }

                        // Claude single-mode cleaning runs in-process so the
                        // next URL waits for it to finish. A clean failure leaves
                        // the raw transcript as the source of truth, but is
                        // surfaced as a warning rather than a silent success.
                        if matches!(auto_clean_selector, AutoCleanTranscript::Claude) {
                            let paths_slice = std::slice::from_ref(&transcript_path);
                            let cleaned = crate::postprocess::clean_transcript_with_claude(
                                emitter,
                                cancel_token,
                                &url_input.item_id,
                                &filestem,
                                paths_slice,
                                &settings.claude_model_effort,
                            )?;
                            if !cleaned {
                                clean_warning = true;
                            }
                        }

                        summarize_source = Some(transcript_path.clone());
                        summarize_stem = filestem.clone();
                    }
                }
                let transcript_status = if !transcript_ok {
                    "failed"
                } else if clean_warning {
                    // Transcript saved, but the requested cleaning didn't complete.
                    "warning"
                } else {
                    "completed"
                };
                self.emit_artifact_status(
                    emitter,
                    &url_input.item_id,
                    ArtifactKind::Transcript,
                    transcript_status,
                )?;
            }

            // ---- Summarize step (runs in-process so the next URL waits) ----
            if job_config.summarize && transcript_ok {
                if let Some(transcript_path) = summarize_source.as_ref() {
                    let summary_output = crate::postprocess::summary_output_path(transcript_path);
                    crate::postprocess::summarize_transcript(
                        emitter,
                        cancel_token,
                        settings,
                        &url_input.item_id,
                        &summarize_stem,
                        transcript_path,
                        &summary_output,
                    )?;
                } else {
                    // No canonical transcript to summarize (e.g. both-mode
                    // cleaning failed, or both-mode without Claude cleaning).
                    // Report the skip rather than leaving the summary pending.
                    emitter.emit(BackendEvent::Log {
                        message: format!(
                            "Summary skipped for {}: no single cleaned transcript available",
                            url_input.item_id
                        ),
                    })?;
                    self.emit_artifact_status(
                        emitter,
                        &url_input.item_id,
                        ArtifactKind::Summary,
                        "skipped",
                    )?;
                }
            }

            if clean_warning {
                clean_warnings += 1;
            }

            emitter.emit(BackendEvent::StatusChange {
                item_id: url_input.item_id.clone(),
                status: if video_ok && audio_ok && transcript_ok {
                    JobStatus::Completed
                } else {
                    JobStatus::Failed
                },
            })?;
        }

        Ok(clean_warnings)
    }

    #[allow(clippy::too_many_arguments)]
    fn process_direct_audio_urls(
        &self,
        urls: &[String],
        job_config: &JobConfig,
        settings: &Settings,
        adapter_request: &AdapterRequest,
        emitter: &EventEmitter,
        cancel_token: &CancellationToken,
        paths: &RuntimePaths,
        modes: EffectiveModes,
        model: &str,
        gpu: &str,
        ffmpeg_path: Option<&str>,
        whisperx_path: Option<String>,
        _whisper_bridge: &Path,
        auto_clean_selector: AutoCleanTranscript,
        verbose: bool,
        cleaner_bridge: Option<&Path>,
    ) -> Result<usize> {
        let parsed: Vec<UrlInput> = urls
            .iter()
            .map(|url| UrlInput {
                item_id: url.clone(),
                url: url.clone(),
                clips: Vec::new(),
            })
            .collect();
        self.process_urls(
            &parsed,
            job_config,
            settings,
            adapter_request,
            emitter,
            cancel_token,
            paths,
            modes,
            model,
            gpu,
            &TranscriptSource::Whisper,
            auto_clean_selector,
            ffmpeg_path,
            whisperx_path,
            settings.browser.clone(),
            None,
            None,
            verbose,
            cleaner_bridge,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn process_transcript_files(
        &self,
        transcript_files: &[PathBuf],
        job_config: &JobConfig,
        settings: &Settings,
        adapter_request: &AdapterRequest,
        emitter: &EventEmitter,
        cancel_token: &CancellationToken,
        paths: &RuntimePaths,
        verbose: bool,
        cleaner_bridge: &Path,
    ) -> Result<usize> {
        let total = transcript_files.len() as i64;
        let stage = "cleaning transcript".to_owned();
        let skip_inputs: HashSet<String> = job_config
            .skip_inputs
            .clone()
            .unwrap_or_default()
            .into_iter()
            .collect();
        let manual_inputs: HashSet<String> = job_config
            .manual_clean_inputs
            .clone()
            .unwrap_or_default()
            .into_iter()
            .collect();
        let auto_clean_selector = AutoCleanTranscript::parse(
            job_config
                .auto_clean_transcript
                .as_deref()
                .unwrap_or(&settings.auto_clean_transcript),
        );

        // Pre-flight: if any file will need Ollama, ensure it's running once
        let needs_ollama =
            auto_clean_selector == AutoCleanTranscript::Ollama || !manual_inputs.is_empty();
        let ollama_available = !needs_ollama || crate::ollama::ensure_ollama_running(emitter);
        // Items whose transcript was kept but whose Claude cleaning did not
        // fully succeed (reported as a `warning`, tallied into the job summary).
        let mut clean_warnings = 0usize;

        for (index_value, transcript_path) in transcript_files.iter().enumerate() {
            self.ensure_not_cancelled(cancel_token)?;
            let item_id = transcript_path.to_string_lossy().to_string();
            let current = (index_value + 1) as i64;
            let percent = if total > 0 {
                (current as f64 / total as f64) * 100.0
            } else {
                100.0
            };
            emitter.emit(BackendEvent::StageCount {
                stage_name: stage.clone(),
                current,
                total,
            })?;
            emitter.emit(BackendEvent::Progress {
                percent,
                stage: stage.clone(),
            })?;
            emitter.emit(BackendEvent::StatusChange {
                item_id: item_id.clone(),
                status: JobStatus::Running,
            })?;
            self.emit_artifact_status(
                emitter,
                &item_id,
                ArtifactKind::Filestem,
                &sanitize_filename(
                    transcript_path
                        .file_stem()
                        .and_then(|value| value.to_str())
                        .unwrap_or("transcript"),
                ),
            )?;
            self.emit_artifact_status(emitter, &item_id, ArtifactKind::Video, "skipped")?;
            self.emit_artifact_status(emitter, &item_id, ArtifactKind::Audio, "skipped")?;

            if skip_inputs.contains(&item_id) {
                self.emit_artifact_status(emitter, &item_id, ArtifactKind::Transcript, "skipped")?;
                emitter.emit(BackendEvent::StatusChange {
                    item_id,
                    status: JobStatus::Completed,
                })?;
                continue;
            }

            self.emit_artifact_status(emitter, &item_id, ArtifactKind::Transcript, "running")?;

            // ---- Cleaning step (Off / Claude / Ollama) ----
            let python_cleaner = if manual_inputs.contains(&item_id) {
                Some("ollama".to_owned())
            } else {
                cleaner_for_selector(&auto_clean_selector)
            };
            let want_claude_clean = !manual_inputs.contains(&item_id)
                && matches!(auto_clean_selector, AutoCleanTranscript::Claude);

            let filestem_str = transcript_path
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or("transcript")
                .to_owned();
            let mut clean_warning = false;

            let transcript_ok = if let Some(cleaner_name) = python_cleaner.clone() {
                if cleaner_name == "ollama" && !ollama_available {
                    // Ollama needed but unavailable — leave the transcript untouched
                    // and treat as success (matches prior behavior).
                    true
                } else {
                    let clean_output = paths.temp_dir.join(format!("{filestem_str}_cleaned.txt"));
                    emitter.emit(BackendEvent::Log {
                        message: format!(
                            "Transcript cleaning started for {} using {}",
                            item_id, cleaner_name
                        ),
                    })?;
                    let cleaned =
                        self.transcript_cleaner
                            .clean_transcript(TranscriptCleanRequest {
                                python_executable: adapter_request.python_executable.clone(),
                                bridge_script: cleaner_bridge.to_path_buf(),
                                transcript_path: transcript_path.clone(),
                                output_path: clean_output.clone(),
                                cleaner: cleaner_name,
                                log_file_path: Some(paths.temp_dir.join("avtget_debug.log")),
                            });
                    let ok = match cleaned {
                        Ok(outcome) => {
                            emit_transcript_cleaning_details(emitter, &item_id, &outcome)?;
                            clean_output.exists()
                        }
                        Err(err) => {
                            emitter.emit(BackendEvent::Log {
                                message: format!(
                                    "Transcript cleaning failed for {}: {}",
                                    item_id, err
                                ),
                            })?;
                            false
                        }
                    };
                    if ok {
                        fs::copy(&clean_output, transcript_path)?;
                    }
                    ok
                }
            } else if want_claude_clean {
                let paths_slice = std::slice::from_ref(transcript_path);
                let cleaned = crate::postprocess::clean_transcript_with_claude(
                    emitter,
                    cancel_token,
                    &item_id,
                    &filestem_str,
                    paths_slice,
                    &settings.claude_model_effort,
                )?;
                if !cleaned {
                    // Claude clean failed — keep the transcript as the source of
                    // truth for summarize (it's still readable, just uncleaned),
                    // but surface a warning rather than a clean success.
                    clean_warning = true;
                }
                true
            } else {
                emitter.emit(BackendEvent::Log {
                    message: format!(
                        "Transcript cleaning skipped for {} (auto-clean disabled)",
                        item_id
                    ),
                })?;
                true
            };

            let transcript_status = if !transcript_ok {
                "failed"
            } else if clean_warning {
                "warning"
            } else {
                "completed"
            };
            self.emit_artifact_status(
                emitter,
                &item_id,
                ArtifactKind::Transcript,
                transcript_status,
            )?;

            // ---- Summarize step (runs in-process so the next item waits) ----
            if job_config.summarize && transcript_ok {
                let summary_output = crate::postprocess::summary_output_path(transcript_path);
                crate::postprocess::summarize_transcript(
                    emitter,
                    cancel_token,
                    settings,
                    &item_id,
                    &filestem_str,
                    transcript_path,
                    &summary_output,
                )?;
            }

            if clean_warning {
                clean_warnings += 1;
            }

            emitter.emit(BackendEvent::StatusChange {
                item_id,
                status: if transcript_ok {
                    JobStatus::Completed
                } else {
                    JobStatus::Failed
                },
            })?;
        }

        Ok(clean_warnings)
    }

    fn download_audio_for_url(
        &self,
        url: &str,
        filestem: &str,
        python_executable: &str,
        ffmpeg_path: Option<&str>,
        verbose: bool,
        paths: &RuntimePaths,
    ) -> Result<Option<PathBuf>> {
        let output_template = paths.temp_dir.join(format!("{filestem}_audio.%(ext)s"));
        let downloaded = self.ytdlp.download_media(
            python_executable,
            url,
            "bestaudio/best",
            &output_template.to_string_lossy(),
            ffmpeg_path,
            verbose,
        );
        if downloaded.is_err() {
            return Ok(None);
        }
        Ok(find_file_with_stem(
            &paths.temp_dir,
            &format!("{filestem}_audio"),
        ))
    }

    fn copy_downloaded_video_to_storage(
        &self,
        paths: &RuntimePaths,
        filestem: &str,
        emitter: &EventEmitter,
    ) -> Result<bool> {
        let Some(source) = find_file_with_stem(&paths.temp_dir, filestem) else {
            return Ok(false);
        };
        let Some(file_name) = source.file_name() else {
            return Ok(false);
        };
        let target = paths.video_dir.join(file_name);
        emitter.emit(BackendEvent::Log {
            message: format!("Copying video to {}", target.display()),
        })?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(&source, &target)?;
        Ok(target.exists())
    }

    /// Download an audio file directly via curl (for resolved podcast URLs where
    /// yt-dlp is not applicable). Infers the file extension from the URL.
    fn download_direct_audio(
        &self,
        url: &str,
        filestem: &str,
        paths: &RuntimePaths,
    ) -> Result<Option<PathBuf>> {
        // Infer extension from URL (strip query/fragment first)
        let clean_url = url.split('?').next().unwrap_or(url);
        let clean_url = clean_url.split('#').next().unwrap_or(clean_url);
        let ext = Path::new(clean_url)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("mp3");
        let output_path = paths.temp_dir.join(format!("{filestem}_audio.{ext}"));
        fs::create_dir_all(&paths.temp_dir)?;

        let result = Command::new("curl")
            .args([
                "-sL",
                "--max-time",
                "300",
                "-o",
                &output_path.to_string_lossy(),
                url,
            ])
            .output();

        match result {
            Ok(output) if output.status.success() && output_path.exists() => Ok(Some(output_path)),
            _ => Ok(None),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn transcribe_audio_path(
        &self,
        audio_path: &Path,
        filestem: &str,
        transcript_path: &Path,
        adapter_request: &AdapterRequest,
        model: &str,
        gpu: &str,
        ffmpeg_path: Option<&str>,
        whisperx_path: Option<String>,
        clips: Vec<ClipRange>,
        clips_full_output: bool,
        paths: &RuntimePaths,
        emitter: &EventEmitter,
        verbose: bool,
    ) -> Result<bool> {
        let Some(bridge_script) = adapter_request.python_whisper_bridge.as_deref() else {
            emitter.emit(BackendEvent::Log {
                message: "WhisperX transcription failed: bridge script not available".to_owned(),
            })?;
            return Ok(false);
        };
        if !audio_path.exists() {
            emitter.emit(BackendEvent::Log {
                message: format!(
                    "WhisperX transcription failed: audio not found at {}",
                    audio_path.display()
                ),
            })?;
            return Ok(false);
        }

        // Convert to 16 KHz mono WAV for optimal WhisperX input
        let whisper_wav = paths.temp_dir.join(format!("{filestem}_whisper.wav"));
        let transcription_source = if !whisper_wav.exists() {
            emitter.emit(BackendEvent::Log {
                message: whisper_conversion_message(audio_path),
            })?;
            self.ffmpeg
                .to_whisper_wav(audio_path, &whisper_wav, ffmpeg_path, verbose)
                .unwrap_or(false);
            if whisper_wav.exists() {
                whisper_wav
            } else {
                audio_path.to_path_buf()
            }
        } else {
            whisper_wav
        };

        let request = WhisperBridgeRequest {
            python_executable: adapter_request.python_executable.clone(),
            bridge_script: bridge_script.to_path_buf(),
            audio_path: transcription_source,
            output_dir: paths.transcripts_dir.clone(),
            temp_dir: paths.temp_dir.clone(),
            output_filestem: filestem.to_owned(),
            model: model.to_owned(),
            gpu: gpu.to_owned(),
            clips,
            clips_full_output,
            whisperx_path,
            ffmpeg_path: ffmpeg_path.map(str::to_owned),
        };
        match self.whisper.transcribe(request) {
            Ok(()) => {
                let exists = transcript_path.exists();
                if !exists {
                    emitter.emit(BackendEvent::Log {
                        message: format!(
                            "WhisperX transcription failed: output not found at {}",
                            transcript_path.display()
                        ),
                    })?;
                }
                Ok(exists)
            }
            Err(err) => {
                emitter.emit(BackendEvent::Log {
                    message: format!("WhisperX transcription failed: {}", err),
                })?;
                Ok(false)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn maybe_auto_clean_transcript(
        &self,
        transcript_path: &Path,
        adapter_request: &AdapterRequest,
        settings: &Settings,
        paths: &RuntimePaths,
        selector: AutoCleanTranscript,
        cleaner_bridge: Option<&Path>,
        verbose: bool,
        emitter: &EventEmitter,
    ) -> Result<Option<TranscriptCleanOutcome>> {
        let Some(cleaner) = cleaner_for_selector(&selector) else {
            return Ok(None);
        };
        if selector == AutoCleanTranscript::Ollama && !crate::ollama::ensure_ollama_running(emitter)
        {
            return Ok(None);
        }
        let Some(bridge) = cleaner_bridge else {
            return Ok(None);
        };
        if !transcript_path.exists() {
            return Ok(None);
        }
        let clean_output = paths.temp_dir.join(format!(
            "{}_auto_clean.txt",
            transcript_path
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or("transcript")
        ));
        let outcome = self
            .transcript_cleaner
            .clean_transcript(TranscriptCleanRequest {
                python_executable: adapter_request.python_executable.clone(),
                bridge_script: bridge.to_path_buf(),
                transcript_path: transcript_path.to_path_buf(),
                output_path: clean_output.clone(),
                cleaner,
                log_file_path: Some(paths.temp_dir.join("avtget_debug.log")),
            })?;
        if !clean_output.exists() {
            return Err(BackendError::Process(format!(
                "transcript cleaner did not write output file: {}",
                clean_output.display()
            )));
        }
        fs::copy(clean_output, transcript_path)?;
        Ok(Some(outcome))
    }

    #[allow(clippy::too_many_arguments)]
    fn download_captions_as_text(
        &self,
        url: &str,
        filestem: &str,
        transcript_path: &Path,
        python_executable: &str,
        ffmpeg_path: Option<&str>,
        verbose: bool,
        paths: &RuntimePaths,
    ) -> Result<bool> {
        let output_template = paths.temp_dir.join(format!("{filestem}.%(ext)s"));
        let _ = self.ytdlp.download_subtitles(
            python_executable,
            url,
            &output_template.to_string_lossy(),
            ffmpeg_path,
            verbose,
        );
        let Some(subtitle_path) = find_best_subtitle_path(&paths.temp_dir, filestem) else {
            return Ok(false);
        };
        subtitle_to_text(&subtitle_path, transcript_path)?;
        Ok(transcript_path.exists())
    }

    /// Reconcile the two "both"-mode transcripts via Claude cleaning, with a
    /// graceful fallback when only one source was retrieved.
    ///
    /// - both present → dual-input clean → merged `{filestem}{suffix}.txt`
    ///   (the skill strips the `-yt`/`-whisper` suffix and deletes both inputs)
    /// - one present  → rename it to the canonical name and single-clean it
    ///   in place (e.g. Overcast/podcast links have no captions, only WhisperX)
    /// - neither      → unreachable; the caller guards on `transcript_ok`
    ///
    /// Returns the canonical transcript to summarize (when one was produced or
    /// retained) and whether Claude cleaning fully succeeded.
    #[allow(clippy::too_many_arguments)]
    fn clean_both_mode(
        &self,
        emitter: &EventEmitter,
        cancel_token: &CancellationToken,
        item_id: &str,
        filestem: &str,
        clip_suffix: &str,
        yt_transcript_path: &Path,
        yt_ok: bool,
        whisper_transcript_path: &Path,
        whisper_ok: bool,
        paths: &RuntimePaths,
        effort: &str,
    ) -> Result<BothModeClean> {
        let canonical = paths
            .transcripts_dir
            .join(format!("{filestem}{clip_suffix}.txt"));

        if yt_ok && whisper_ok {
            // Both sources present → merge into a single canonical transcript.
            let dual_paths = [
                yt_transcript_path.to_path_buf(),
                whisper_transcript_path.to_path_buf(),
            ];
            let cleaned = crate::postprocess::clean_transcript_with_claude(
                emitter, cancel_token, item_id, filestem, &dual_paths, effort,
            )?;
            if cleaned && canonical.exists() {
                return Ok(BothModeClean {
                    summarize_source: Some(canonical),
                    cleaned: true,
                });
            }
            emitter.emit(BackendEvent::Log {
                message: format!(
                    "Both-mode cleaning failed for {item_id}: raw -yt/-whisper transcripts kept; summary skipped"
                ),
            })?;
            return Ok(BothModeClean {
                summarize_source: None,
                cleaned: false,
            });
        }

        // Graceful fallback: only one source was retrieved. Clean that single
        // transcript instead of silently skipping cleaning.
        let (available, which) = if yt_ok {
            (yt_transcript_path, "captions")
        } else {
            (whisper_transcript_path, "whisper")
        };
        emitter.emit(BackendEvent::Log {
            message: format!(
                "Both mode degraded to single source for {item_id}: only {which} retrieved — cleaning it alone"
            ),
        })?;

        // Rename the surviving transcript to the canonical (suffix-free) name so
        // the cleaned output matches single-source naming, then clean in place.
        let clean_target = if available != canonical.as_path() {
            match fs::rename(available, &canonical) {
                Ok(()) => canonical.clone(),
                Err(err) => {
                    emitter.emit(BackendEvent::Log {
                        message: format!(
                            "Could not rename {} to {} ({err}); cleaning in place",
                            available.display(),
                            canonical.display()
                        ),
                    })?;
                    available.to_path_buf()
                }
            }
        } else {
            canonical.clone()
        };

        let single = std::slice::from_ref(&clean_target);
        let cleaned = crate::postprocess::clean_transcript_with_claude(
            emitter, cancel_token, item_id, filestem, single, effort,
        )?;
        // The (raw or cleaned) transcript exists at clean_target either way and
        // is worth summarizing.
        let summarize_source = clean_target.exists().then(|| clean_target.clone());
        if !cleaned {
            emitter.emit(BackendEvent::Log {
                message: format!(
                    "Cleaning failed for {item_id}: raw transcript saved at {}",
                    clean_target.display()
                ),
            })?;
        }
        Ok(BothModeClean {
            summarize_source,
            cleaned,
        })
    }

    fn ensure_not_cancelled(&self, cancel_token: &CancellationToken) -> Result<()> {
        if cancel_token.is_cancelled() {
            return Err(BackendError::Cancelled);
        }
        Ok(())
    }

    fn emit_artifact_status(
        &self,
        emitter: &EventEmitter,
        item_id: &str,
        artifact: ArtifactKind,
        status: &str,
    ) -> Result<()> {
        emitter.emit(BackendEvent::ArtifactStatus {
            item_id: item_id.to_owned(),
            artifact,
            status: status.to_owned(),
        })
    }
}

#[derive(Clone, Copy)]
struct EffectiveModes {
    video: bool,
    audio: bool,
    transcript: bool,
}

/// Outcome of reconciling the two "both"-mode transcripts via Claude cleaning.
struct BothModeClean {
    /// Canonical transcript to summarize, if one was produced or retained.
    summarize_source: Option<PathBuf>,
    /// True only when Claude cleaning fully succeeded (dual merge or the
    /// single-source fallback). False means the raw transcript(s) were kept.
    cleaned: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TranscriptSource {
    Captions,
    Whisper,
    /// Run captions + whisper side-by-side and write each to its own suffixed
    /// transcript file (`-yt.txt` and `-whisper.txt`). When Claude cleaning is
    /// selected, the clean-transcript skill reconciles both into a single
    /// `{filestem}.txt`; if only one source is retrieved (e.g. captions are
    /// unavailable for a podcast link), cleaning gracefully falls back to that
    /// single transcript. With cleaning off, both files are left in place.
    Both,
}

#[derive(Debug, Clone)]
struct UrlInput {
    item_id: String,
    url: String,
    clips: Vec<ClipRange>,
}

#[derive(Default)]
struct ParsedInputs {
    urls: Vec<UrlInput>,
    transcript_files: Vec<PathBuf>,
    direct_audio_urls: Vec<String>,
    unsupported_inputs: Vec<String>,
    timeframe_days: Option<i64>,
    channel_url: Option<String>,
}

#[derive(Debug, Clone)]
struct RuntimePaths {
    storage_root: PathBuf,
    video_dir: PathBuf,
    audio_dir: PathBuf,
    transcripts_dir: PathBuf,
    temp_dir: PathBuf,
}

impl RuntimePaths {
    fn resolve(config_path: &Path, settings: &Settings) -> Self {
        let storage_root = resolve_relative_to_config(config_path, &settings.storage_directory);
        let temp_dir = resolve_relative_to_config(config_path, &settings.temp_directory);
        Self {
            video_dir: storage_root.join("video"),
            audio_dir: storage_root.join("audio"),
            transcripts_dir: storage_root.join("transcripts"),
            storage_root,
            temp_dir,
        }
    }

    fn ensure_directories(&self) -> Result<()> {
        fs::create_dir_all(&self.storage_root)?;
        fs::create_dir_all(&self.video_dir)?;
        fs::create_dir_all(&self.audio_dir)?;
        fs::create_dir_all(&self.transcripts_dir)?;
        fs::create_dir_all(&self.temp_dir)?;
        Ok(())
    }
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
    if configured.is_empty() {
        "medium".to_owned()
    } else {
        configured.to_owned()
    }
}

fn choose_gpu(job_config: &JobConfig) -> String {
    job_config
        .gpu
        .as_ref()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "0".to_owned())
}

fn choose_transcript_source(job_config: &JobConfig, settings: &Settings) -> TranscriptSource {
    let value = job_config
        .transcript_source
        .as_ref()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            settings
                .default_transcript_source
                .trim()
                .to_ascii_lowercase()
        });
    match value.as_str() {
        "whisper" => TranscriptSource::Whisper,
        "both" => TranscriptSource::Both,
        _ => TranscriptSource::Captions,
    }
}

fn resolve_tool_path(config_path: &Path, configured: &str) -> Option<String> {
    let trimmed = configured.trim();
    if trimmed.is_empty() || trimmed == "." {
        return None;
    }
    Some(
        resolve_relative_to_config(config_path, trimmed)
            .to_string_lossy()
            .into_owned(),
    )
}

fn resolve_bridge_script(
    config_path: &Path,
    bridge_hint: Option<&Path>,
    script_name: &str,
) -> Option<PathBuf> {
    if let Some(hint) = bridge_hint {
        let candidate = if hint.is_dir() {
            hint.join(script_name)
        } else {
            hint.parent().map(|parent| parent.join(script_name))?
        };
        if candidate.exists() {
            return Some(candidate);
        }
    }
    let config_dir = config_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let candidates = [
        config_dir.join("backend").join(script_name),
        config_dir.join(script_name),
    ];
    candidates.into_iter().find(|candidate| candidate.exists())
}

fn stage_description(modes: &EffectiveModes, model: &str, settings: &Settings) -> String {
    if modes.video {
        return "video".to_owned();
    }
    if modes.audio {
        return "audio".to_owned();
    }
    if modes.transcript {
        let source = settings.default_transcript_source.trim();
        if source.eq_ignore_ascii_case("captions") {
            return "transcript -".to_owned();
        }
        if source.eq_ignore_ascii_case("both") {
            return format!("transcript - both (yt + whisperx {model}) -");
        }
        return format!("transcript - whisperx {model} -");
    }
    "processing".to_owned()
}

fn parse_timeframe_days(raw: &str) -> Option<i64> {
    let value = raw.trim().to_ascii_lowercase();
    if value.is_empty() {
        return None;
    }
    let (digits, multiplier) = if let Some(value) = value.strip_suffix('d') {
        (value, 1)
    } else if let Some(value) = value.strip_suffix('w') {
        (value, 7)
    } else if let Some(value) = value.strip_suffix('m') {
        (value, 30)
    } else if let Some(value) = value.strip_suffix('y') {
        (value, 365)
    } else {
        (value.as_str(), 1)
    };
    let base: i64 = digits.parse().ok()?;
    if base < 0 {
        None
    } else {
        Some(base * multiplier)
    }
}

fn looks_like_url(value: &str) -> bool {
    let lowered = value.trim().to_ascii_lowercase();
    lowered.starts_with("http://") || lowered.starts_with("https://") || lowered.starts_with("www.")
}

fn normalize_url(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.to_ascii_lowercase().starts_with("www.") {
        format!("https://{trimmed}")
    } else {
        trimmed.to_owned()
    }
}

fn is_channel_url(value: &str) -> bool {
    let lowered = normalize_url(value).to_ascii_lowercase();
    lowered.contains("/@")
        || lowered.contains("/c/")
        || lowered.contains("/channel/")
        || lowered.contains("/user/")
}

fn is_podcast_url(value: &str) -> bool {
    let lowered = normalize_url(value).to_ascii_lowercase();
    lowered.contains("overcast.fm/itunes")
        || lowered.ends_with(".xml")
        || lowered.ends_with(".rss")
        || lowered.contains("/feed/")
        || lowered.contains("/rss/")
        || lowered.contains("/podcast.xml")
}

fn is_direct_audio_url(value: &str) -> bool {
    let lowered = normalize_url(value).to_ascii_lowercase();
    if !lowered.starts_with("http://") && !lowered.starts_with("https://") {
        return false;
    }
    let clean = lowered.split('#').next().unwrap_or(lowered.as_str());
    AUDIO_EXTENSIONS
        .iter()
        .any(|extension| clean.ends_with(&format!(".{extension}")))
}

/// Check if URL is an Overcast single episode link (e.g. `overcast.fm/+_pp71azAw`).
fn is_overcast_episode_url(value: &str) -> bool {
    let lowered = normalize_url(value).to_ascii_lowercase();
    lowered.contains("overcast.fm/+")
}

/// Metadata resolved from an Overcast episode page.
struct OvercastEpisode {
    /// Direct MP3 URL extracted from the `twitter:player:stream` meta tag.
    audio_url: String,
    /// Episode title (may include podcast name, separated by ` — `).
    title: Option<String>,
    /// Podcast/show name parsed from the og:title (the part after the last ` — `).
    channel: Option<String>,
}

/// Fetch the Overcast episode page and extract the direct audio URL and metadata
/// from HTML meta tags. Uses `curl` to avoid adding an HTTP client dependency.
fn resolve_overcast_episode(url: &str) -> Option<OvercastEpisode> {
    let output = Command::new("curl")
        .args(["-sL", "--max-time", "15", url])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let html = String::from_utf8_lossy(&output.stdout);

    // Extract twitter:player:stream → direct audio URL
    let audio_url = extract_meta_content(&html, "twitter:player:stream")?;
    // Strip fragment (e.g. #t=0) from the audio URL
    let audio_url = audio_url.split('#').next().unwrap_or(&audio_url).to_owned();

    // Extract og:title → "Episode Title — Podcast Name"
    let og_title = extract_meta_content(&html, "og:title");
    let (title, channel) = match og_title {
        Some(ref full_title) => {
            // Overcast og:title format: "Episode Title — Podcast Name"
            // The ` — ` (em dash with spaces) separates episode from show.
            if let Some(pos) = full_title.rfind(" \u{2014} ") {
                let episode = full_title[..pos].trim().to_owned();
                let show = full_title[pos + 4..].trim().to_owned();
                (Some(episode), Some(show))
            } else {
                (Some(full_title.clone()), None)
            }
        }
        None => (None, None),
    };

    Some(OvercastEpisode {
        audio_url,
        title,
        channel,
    })
}

/// Extract the `content` attribute value from a `<meta>` tag with the given
/// `name` or `property` attribute. Handles both `name="X" content="Y"` and
/// `content="Y" name="X"` orderings.
fn extract_meta_content(html: &str, tag_name: &str) -> Option<String> {
    // Search for the tag_name in meta tags. Overcast uses name= (not property=).
    // We scan for the tag_name, then find the enclosing <meta ...> and extract content.
    let needle = format!("\"{}\"", tag_name);
    let mut search_start = 0;
    while let Some(name_pos) = html[search_start..].find(&needle) {
        let abs_pos = search_start + name_pos;
        // Walk backwards to find the opening `<meta`
        let before = &html[..abs_pos];
        if let Some(meta_start) = before.rfind("<meta") {
            let after_meta = &html[meta_start..];
            if let Some(end) = after_meta.find('>') {
                let meta_tag = &after_meta[..=end];
                // Extract content="..." from this meta tag
                if let Some(content_start) = meta_tag.find("content=\"") {
                    let value_start = content_start + 9; // len("content=\"")
                    if let Some(value_end) = meta_tag[value_start..].find('"') {
                        let raw = &meta_tag[value_start..value_start + value_end];
                        // Decode common HTML entities
                        let decoded = raw
                            .replace("&amp;", "&")
                            .replace("&mdash;", "\u{2014}")
                            .replace("&ndash;", "\u{2013}")
                            .replace("&rsquo;", "\u{2019}")
                            .replace("&lsquo;", "\u{2018}")
                            .replace("&rdquo;", "\u{201D}")
                            .replace("&ldquo;", "\u{201C}")
                            .replace("&hellip;", "\u{2026}")
                            .replace("&lt;", "<")
                            .replace("&gt;", ">");
                        return Some(decoded);
                    }
                }
            }
        }
        search_start = abs_pos + needle.len();
    }
    None
}

fn push_url_like_input(raw: &str, clips: Vec<ClipRange>, parsed: &mut ParsedInputs) {
    let normalized = normalize_url(raw);
    if is_direct_audio_url(&normalized) {
        parsed.direct_audio_urls.push(normalized);
        return;
    }
    if is_podcast_url(&normalized) {
        parsed.unsupported_inputs.push(raw.to_owned());
        return;
    }
    parsed.urls.push(UrlInput {
        item_id: raw.to_owned(),
        url: normalized,
        clips,
    });
}

fn clips_for_index(job_config: &JobConfig, index: usize) -> Vec<ClipRange> {
    job_config
        .clip_timestamps
        .as_ref()
        .and_then(|all| all.get(index).cloned())
        .unwrap_or_default()
}

fn cleaner_for_selector(selector: &AutoCleanTranscript) -> Option<String> {
    match selector {
        AutoCleanTranscript::Off => None,
        // Claude cleaning never routes through the Python bridge — it runs
        // in-process as a two-turn `claude -p --resume` against the
        // clean-transcript skill (postprocess::clean_transcript_with_claude),
        // so returning None makes the orchestrator skip the bridge entirely.
        AutoCleanTranscript::Claude => None,
        AutoCleanTranscript::Ollama => Some("ollama".to_owned()),
    }
}

fn emit_transcript_cleaning_details(
    emitter: &EventEmitter,
    item_label: &str,
    outcome: &TranscriptCleanOutcome,
) -> Result<()> {
    let cleaner = outcome.cleaner.as_deref().unwrap_or("unknown");
    let provider = outcome.provider.as_deref().unwrap_or("unknown");
    let shards_total = outcome.shards_total.unwrap_or(1);
    let used_sharding = outcome.used_sharding.unwrap_or(shards_total > 1);
    let shard_mode = if used_sharding {
        format!("sharded ({shards_total} shards)")
    } else {
        "single pass (no sharding)".to_owned()
    };
    let size_detail = match (outcome.raw_chars, outcome.cleaned_chars) {
        (Some(raw), Some(cleaned)) => format!(", chars: raw={raw}, cleaned={cleaned}"),
        _ => String::new(),
    };
    emitter.emit(BackendEvent::Log {
        message: format!(
            "Transcript cleaning details for {}: provider={}, cleaner={}, {}{}",
            item_label, provider, cleaner, shard_mode, size_detail
        ),
    })?;

    Ok(())
}

fn fallback_filestem_for_input(raw: &str) -> String {
    let value = raw.trim();
    if value.is_empty() {
        return "unknown_input".to_owned();
    }
    if looks_like_url(value) {
        let clean = normalize_url(value);
        // For YouTube URLs, extract the video ID from the `v=` query parameter
        // so that each video gets a unique fallback filestem instead of "watch".
        if let Some(video_id) = extract_youtube_video_id(&clean) {
            return sanitize_filename(&video_id);
        }
        let without_fragment = clean.split('#').next().unwrap_or(clean.as_str());
        let without_query = without_fragment
            .split('?')
            .next()
            .unwrap_or(without_fragment);
        let candidate = without_query.rsplit('/').next().unwrap_or("unknown_input");
        return sanitize_filename(
            Path::new(candidate)
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or("unknown_input"),
        );
    }
    sanitize_filename(
        Path::new(value)
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or(value),
    )
}

/// Extract the YouTube video ID from a URL.
/// Handles `youtube.com/watch?v=ID`, `youtu.be/ID`, and `youtube.com/embed/ID`.
fn extract_youtube_video_id(url: &str) -> Option<String> {
    let lowered = url.to_ascii_lowercase();
    if lowered.contains("youtube.com") || lowered.contains("youtu.be") {
        // Try `v=` query parameter (most common)
        if let Some(pos) = url.find("v=") {
            let after_v = &url[pos + 2..];
            let id: String = after_v
                .chars()
                .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_')
                .collect();
            if !id.is_empty() {
                return Some(id);
            }
        }
        // Try youtu.be/ID short URL
        if let Some(pos) = url.find("youtu.be/") {
            let after = &url[pos + 9..];
            let id: String = after
                .chars()
                .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_')
                .collect();
            if !id.is_empty() {
                return Some(id);
            }
        }
        // Try /embed/ID
        if let Some(pos) = url.find("/embed/") {
            let after = &url[pos + 7..];
            let id: String = after
                .chars()
                .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_')
                .collect();
            if !id.is_empty() {
                return Some(id);
            }
        }
    }
    None
}

fn generate_filename_from_template(
    template: &str,
    channel: Option<&str>,
    title: Option<&str>,
    counter: usize,
    source: &str,
) -> String {
    let channel_value = sanitize_or_fallback(channel, "Unknown");
    let title_value = sanitize_or_fallback(title, "Unknown");
    let source_value = sanitize_or_fallback(Some(source), "unknown");
    let template_value = if template.trim().is_empty() {
        "%channelname - %videotitle"
    } else {
        template
    };

    let mut filename = template_value.to_owned();
    filename = filename.replace("%channelname", &channel_value);
    filename = filename.replace("%videotitle", &title_value);
    filename = filename.replace("%date", &today_yyyymmdd());
    filename = filename.replace("%counter", &(counter + 1).to_string());
    filename = filename.replace("%source", &source_value);

    let generated = sanitize_filename(&filename);
    if generated != "unknown_input" {
        return generated;
    }

    sanitize_filename(&format!("{channel_value} - {title_value}"))
}

fn sanitize_or_fallback(value: Option<&str>, fallback: &str) -> String {
    let raw = value.unwrap_or("").trim();
    let chosen = if raw.is_empty() { fallback } else { raw };
    let sanitized = sanitize_filename(chosen);
    if sanitized == "unknown_input" {
        fallback.to_owned()
    } else {
        sanitized
    }
}

fn detect_source_for_url(url: &str) -> &'static str {
    let lowered = normalize_url(url).to_ascii_lowercase();
    if lowered.contains("youtube.com") || lowered.contains("youtu.be") {
        return "youtube";
    }
    if lowered.contains("twitch.tv") {
        return "twitch";
    }
    if lowered.contains("overcast.fm") {
        return "overcast";
    }
    if lowered.ends_with(".xml")
        || lowered.ends_with(".rss")
        || lowered.contains("/feed/")
        || lowered.contains("/rss/")
        || lowered.contains("/podcast.xml")
    {
        return "podcast";
    }
    if is_direct_audio_url(url) {
        return "direct_audio";
    }
    "unknown"
}

fn url_input_type_message(url: &str) -> String {
    let label = if is_channel_url(url) {
        "channel"
    } else if is_overcast_episode_url(url) || is_podcast_url(url) {
        "podcast"
    } else if is_direct_audio_url(url) {
        "audio"
    } else {
        match detect_source_for_url(url) {
            "youtube" => "youtube video",
            "twitch" => "twitch",
            "podcast" | "overcast" => "podcast",
            "direct_audio" => "audio",
            _ => "unknown",
        }
    };
    format!("Input type: URL [{label}]")
}

fn whisper_conversion_message(input_path: &Path) -> String {
    format!(
        "Converting {} to .wav for whisper transcription",
        extension_label(input_path)
    )
}

fn extension_label(path: &Path) -> String {
    path.extension()
        .and_then(|value| value.to_str())
        .map(|value| format!(".{}", value.to_ascii_lowercase()))
        .unwrap_or_else(|| "input".to_owned())
}

fn today_yyyymmdd() -> String {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0));
    let days_since_epoch = (duration.as_secs() / 86_400) as i64;
    let (year, month, day) = civil_from_days(days_since_epoch);
    format!("{year:04}{month:02}{day:02}")
}

// Convert days since Unix epoch (1970-01-01) to Gregorian date (UTC).
fn civil_from_days(days_since_epoch: i64) -> (i32, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if month <= 2 { 1 } else { 0 };
    (year as i32, month as u32, day as u32)
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
    // what actually lands on disk. A title ending in '.' (e.g. "… concerns.")
    // otherwise breaks dual-clean detection — the merged name is predicted as
    // "name..txt" while the skill writes "name.txt".
    let trimmed = output.trim().trim_end_matches(|c: char| c == '.' || c == ' ');
    if trimmed.is_empty() {
        "unknown_input".to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn lowercase_extension(path: &Path) -> String {
    path.extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_default()
}

fn find_file_with_stem(directory: &Path, stem: &str) -> Option<PathBuf> {
    // WhisperX writes its side-effect outputs (.txt/.json/.srt/.tsv/.vtt) into the
    // same directory as the input audio, using the audio file's stem. On a repeat
    // run, both `..._audio.webm` and `..._audio.txt` end up sharing the same stem,
    // and the wrong one would otherwise be handed back to the next whisper job.
    const EXCLUDED_EXTS: &[&str] = &["txt", "json", "srt", "tsv", "vtt"];
    let entries = fs::read_dir(directory).ok()?;
    for entry in entries.filter_map(std::result::Result::ok) {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(file_stem) = path.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        if file_stem != stem {
            continue;
        }
        let ext = path
            .extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        if EXCLUDED_EXTS.contains(&ext.as_str()) {
            continue;
        }
        return Some(path);
    }
    None
}

fn find_best_subtitle_path(temp_dir: &Path, filestem: &str) -> Option<PathBuf> {
    let priorities = [".en-orig", ".en-en", ".en", ""];
    for ext in ["vtt", "srt", "ass"] {
        for suffix in priorities {
            let candidate = temp_dir.join(format!("{filestem}{suffix}.{ext}"));
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    None
}

fn subtitle_to_text(subtitle_path: &Path, transcript_path: &Path) -> Result<()> {
    let file = File::open(subtitle_path)?;
    let reader = BufReader::new(file);
    let mut lines = Vec::new();
    let mut last_line = String::new();
    for line in reader.lines() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.eq_ignore_ascii_case("webvtt")
            || trimmed.chars().all(|ch| ch.is_ascii_digit())
            || trimmed.contains("-->")
        {
            continue;
        }
        let cleaned = strip_html_tags(trimmed);
        let cleaned = cleaned.trim();
        if cleaned.is_empty() || cleaned == last_line {
            continue;
        }
        last_line = cleaned.to_owned();
        lines.push(cleaned.to_owned());
    }
    if let Some(parent) = transcript_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut output = File::create(transcript_path)?;
    output.write_all(lines.join("\n").as_bytes())?;
    output.flush()?;
    Ok(())
}

fn strip_html_tags(raw: &str) -> String {
    let mut output = String::with_capacity(raw.len());
    let mut in_tag = false;
    for ch in raw.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => output.push(ch),
            _ => {}
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::sanitize_filename;

    #[test]
    fn sanitize_strips_trailing_dots_and_spaces() {
        // The dual-clean false-failure bug: a title ending in '.' yields a
        // filestem the merged-name prediction can't match. Trailing dots/spaces
        // must be stripped (Windows strips them from real filenames anyway).
        assert_eq!(sanitize_filename("security concerns."), "security concerns");
        assert_eq!(sanitize_filename("trailing space "), "trailing space");
        assert_eq!(sanitize_filename("dots and space. . "), "dots and space");
    }

    #[test]
    fn sanitize_preserves_internal_dots_and_normal_names() {
        assert_eq!(sanitize_filename("v1.0 release"), "v1.0 release");
        assert_eq!(sanitize_filename("Normal Title"), "Normal Title");
    }

    #[test]
    fn sanitize_removes_invalid_chars_and_handles_empty() {
        assert_eq!(sanitize_filename("a/b:c?"), "abc");
        assert_eq!(sanitize_filename("..."), "unknown_input");
        assert_eq!(sanitize_filename("   "), "unknown_input");
    }
}
