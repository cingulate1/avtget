use std::fs;
use std::path::Path;

use avtget_domain::Result;

use super::TempStoreAdapter;

#[derive(Debug, Default)]
pub struct FsTempStorage;

impl TempStoreAdapter for FsTempStorage {
    fn prepare_temp_directory(&self, directory: &Path, _keep_files: bool) -> Result<()> {
        // Temp cleanup is handled by the Tauri shell at app startup
        // (clean_temp_directory). This only ensures the directory exists.
        fs::create_dir_all(directory)?;
        Ok(())
    }
}
