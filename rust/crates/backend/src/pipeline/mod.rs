use std::path::PathBuf;

use avtget_config::{
    effective_keep, load_settings, resolve_config_path, resolve_relative_to_config,
};
use avtget_domain::{BackendError, JobConfig, PipelineState, Result};

use crate::adapters::{AdapterRequest, BackendAdapter, TempStoreAdapter};
use crate::cancel::CancellationToken;
use crate::cli::CliArgs;
use crate::events::EventEmitter;

pub struct BackendPipeline<TStore, TBackend>
where
    TStore: TempStoreAdapter,
    TBackend: BackendAdapter,
{
    state: PipelineState,
    emitter: EventEmitter,
    cancel_token: CancellationToken,
    temp_store: TStore,
    backend_adapter: TBackend,
}

impl<TStore, TBackend> BackendPipeline<TStore, TBackend>
where
    TStore: TempStoreAdapter,
    TBackend: BackendAdapter,
{
    pub fn new(
        emitter: EventEmitter,
        cancel_token: CancellationToken,
        temp_store: TStore,
        backend_adapter: TBackend,
    ) -> Self {
        Self {
            state: PipelineState::Initialized,
            emitter,
            cancel_token,
            temp_store,
            backend_adapter,
        }
    }

    pub fn run(&mut self, args: CliArgs) -> Result<()> {
        match self.execute(args) {
            Ok(()) => Ok(()),
            Err(error) => {
                let _ = self.transition(PipelineState::Failed);
                let _ = self.emitter.emit_job_error(error.to_string());
                Err(error)
            }
        }
    }

    fn execute(&mut self, args: CliArgs) -> Result<()> {
        let job_config = parse_job_config(&args.job_config)?;
        self.transition(PipelineState::ParsedJobConfig)?;

        let config_path = resolve_config_path(job_config.program_dir.as_deref());
        let settings = load_settings(&config_path)?;
        self.transition(PipelineState::SettingsLoaded)?;

        let keep_files = effective_keep(&settings, job_config.keep);
        let temp_directory = resolve_relative_to_config(&config_path, &settings.temp_directory);
        self.temp_store
            .prepare_temp_directory(&temp_directory, keep_files)?;
        // File logging is handled by the Tauri shell, not here.
        self.transition(PipelineState::TempPrepared)?;

        self.transition(PipelineState::Delegating)?;
        let adapter_outcome = self.backend_adapter.run(AdapterRequest {
            config_path,
            job_config_json: args.job_config,
            python_executable: args.python_executable,
            python_whisper_bridge: args.python_whisper_bridge.map(PathBuf::from),
            emitter: self.emitter.clone(),
            cancel_token: self.cancel_token.clone(),
        })?;

        if self.cancel_token.is_cancelled() {
            self.transition(PipelineState::Cancelled)?;
            if !adapter_outcome.terminal_event_seen {
                self.emitter.emit_job_finished("Job cancelled by user")?;
            }
            return Ok(());
        }

        if adapter_outcome.exit_code.unwrap_or(0) != 0 {
            self.transition(PipelineState::Failed)?;
            if !adapter_outcome.terminal_event_seen {
                return Err(BackendError::Process(format!(
                    "backend adapter exited with code {:?}",
                    adapter_outcome.exit_code
                )));
            }
            return Ok(());
        }

        self.transition(PipelineState::Finished)?;
        Ok(())
    }

    fn transition(&mut self, next: PipelineState) -> Result<()> {
        if self.state.can_transition_to(next) {
            self.state = next;
            Ok(())
        } else {
            Err(BackendError::Protocol(format!(
                "invalid pipeline transition: {:?} -> {:?}",
                self.state, next
            )))
        }
    }
}

fn parse_job_config(raw: &str) -> Result<JobConfig> {
    let config: JobConfig =
        serde_json::from_str(raw).map_err(|err| BackendError::InvalidJobConfig(err.to_string()))?;
    Ok(config)
}
