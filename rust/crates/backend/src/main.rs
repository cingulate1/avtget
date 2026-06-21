mod adapters;
mod cancel;
mod cli;
mod events;
pub mod ollama;
mod orchestration;
mod pipeline;
mod postprocess;

use adapters::orchestrated_backend::OrchestratedBackendAdapter;
use adapters::temp_storage::FsTempStorage;
use cancel::spawn_stdin_cancel_listener;
use clap::Parser;
use cli::CliArgs;
use events::EventEmitter;
use pipeline::BackendPipeline;

fn main() {
    let args = CliArgs::parse();
    let emitter = EventEmitter::new();
    let cancel_token = cancel::CancellationToken::new();
    let _listener = spawn_stdin_cancel_listener(cancel_token.clone());

    let mut pipeline = BackendPipeline::new(
        emitter.clone(),
        cancel_token,
        FsTempStorage::default(),
        OrchestratedBackendAdapter::default(),
    );

    if pipeline.run(args).is_err() {
        // Errors are emitted in JSON-lines protocol by the pipeline.
    }
}
