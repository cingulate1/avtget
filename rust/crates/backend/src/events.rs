use std::io::{self, BufWriter, Write};
use std::sync::{Arc, Mutex};

use avtget_domain::{BackendError, BackendEvent, Result};

#[derive(Clone)]
pub struct EventEmitter {
    writer: Arc<Mutex<BufWriter<io::Stdout>>>,
}

impl EventEmitter {
    pub fn new() -> Self {
        Self {
            writer: Arc::new(Mutex::new(BufWriter::new(io::stdout()))),
        }
    }

    pub fn emit(&self, event: BackendEvent) -> Result<()> {
        let json = serde_json::to_string(&event)?;

        let mut writer = self
            .writer
            .lock()
            .map_err(|_| BackendError::Protocol("stdout lock poisoned".to_owned()))?;
        writer.write_all(json.as_bytes())?;
        writer.write_all(b"\n")?;
        writer.flush()?;

        Ok(())
    }

    #[allow(dead_code)]
    pub fn emit_log<T: Into<String>>(&self, message: T) -> Result<()> {
        self.emit(BackendEvent::Log {
            message: message.into(),
        })
    }

    pub fn emit_job_error<T: Into<String>>(&self, error: T) -> Result<()> {
        self.emit(BackendEvent::JobError {
            error: error.into(),
        })
    }

    pub fn emit_job_finished<T: Into<String>>(&self, summary: T) -> Result<()> {
        self.emit(BackendEvent::JobFinished {
            summary: summary.into(),
        })
    }

    pub fn emit_raw_json_line(&self, line: &str) -> Result<()> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| BackendError::Protocol("stdout lock poisoned".to_owned()))?;
        writer.write_all(line.as_bytes())?;
        writer.write_all(b"\n")?;
        writer.flush()?;

        Ok(())
    }
}
