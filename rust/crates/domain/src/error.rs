use thiserror::Error;

#[derive(Debug, Error)]
pub enum BackendError {
    #[error("invalid job configuration: {0}")]
    InvalidJobConfig(String),
    #[error("invalid settings: {0}")]
    InvalidSettings(String),
    #[error("io error: {0}")]
    Io(String),
    #[error("process error: {0}")]
    Process(String),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("cancelled")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, BackendError>;

impl From<std::io::Error> for BackendError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value.to_string())
    }
}

impl From<serde_json::Error> for BackendError {
    fn from(value: serde_json::Error) -> Self {
        Self::Protocol(value.to_string())
    }
}
