use clap::Parser;

#[derive(Debug, Clone, Parser)]
#[command(
    name = "avtget-backend",
    version,
    about = "JSON-lines backend protocol bridge for avtget"
)]
pub struct CliArgs {
    #[arg(long = "job-config")]
    pub job_config: String,
    #[arg(long = "python-executable", default_value = "python")]
    pub python_executable: String,
    #[arg(long = "python-whisper-bridge")]
    pub python_whisper_bridge: Option<String>,
}
