//! Pipeline cache persistence (Phase 51).
//!
//! Wraps the Vulkan [`vk::PipelineCache`] opaque blob with save/load around a
//! title's `cache/pipelines/` directory. The blob path is
//! `pipeline-cache.bin`; atomic writes (temp file + rename) prevent
//! corruption on crash. A dirty flag tracks whether any pipeline was created
//! since the last save, so `Drop` can best-effort persist the blob.
//!
//! The pipeline cache is host-independent file I/O when no device is present;
//! the actual `vkCreatePipelineCache` / `vkGetPipelineCacheData` calls happen
//! only when a device is available.

use std::{
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

use thiserror::Error;

/// File name of the persisted pipeline cache blob.
pub const PIPELINE_CACHE_FILE: &str = "pipeline-cache.bin";

/// Errors from pipeline cache operations.
#[derive(Debug, Error)]
pub enum PipelineCacheError {
    #[error("pipeline cache I/O failed for {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("pipeline cache blob is too large ({size} bytes, max {max})")]
    BlobTooLarge { size: u64, max: u64 },
}

impl PipelineCacheError {
    fn io(path: &Path, source: io::Error) -> Self {
        Self::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

/// Maximum pipeline cache blob size (64 MiB).
const MAX_PIPELINE_CACHE_BLOB: u64 = 64 * 1024 * 1024;

/// A pipeline cache blob that can be saved to and loaded from disk.
///
/// This struct owns the opaque blob bytes; it does not hold a Vulkan device
/// handle. Higher-level code passes the blob to `vkCreatePipelineCache` as
/// `initial_data` and retrieves it via `vkGetPipelineCacheData`.
#[derive(Clone, Debug)]
pub struct PipelineCacheBlob {
    /// The opaque pipeline cache data.
    data: Vec<u8>,
    /// Path to persist the blob.
    path: PathBuf,
    /// Whether new pipelines were created since the last save.
    dirty: bool,
}

impl PipelineCacheBlob {
    /// Open or create a pipeline cache at `dir/pipeline-cache.bin`.
    ///
    /// If the file exists and is valid, its contents are loaded as the initial
    /// blob. Otherwise an empty blob is created.
    pub fn open(dir: &Path) -> Result<Self, PipelineCacheError> {
        let path = dir.join(PIPELINE_CACHE_FILE);
        if path.exists() {
            let metadata = fs::symlink_metadata(&path)
                .map_err(|source| PipelineCacheError::io(&path, source))?;
            if metadata.len() > MAX_PIPELINE_CACHE_BLOB {
                return Err(PipelineCacheError::BlobTooLarge {
                    size: metadata.len(),
                    max: MAX_PIPELINE_CACHE_BLOB,
                });
            }
            let mut file =
                fs::File::open(&path).map_err(|source| PipelineCacheError::io(&path, source))?;
            let mut data = Vec::new();
            file.read_to_end(&mut data)
                .map_err(|source| PipelineCacheError::io(&path, source))?;
            Ok(Self {
                data,
                path,
                dirty: false,
            })
        } else {
            fs::create_dir_all(dir).map_err(|source| PipelineCacheError::io(dir, source))?;
            Ok(Self {
                data: Vec::new(),
                path,
                dirty: false,
            })
        }
    }

    /// The opaque pipeline cache data, suitable for passing as `initial_data`
    /// to `vkCreatePipelineCache`.
    #[must_use]
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Whether new pipelines were created since the last save.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Replace the blob with new data from `vkGetPipelineCacheData`.
    pub fn update(&mut self, new_data: Vec<u8>) {
        self.data = new_data;
        self.dirty = true;
    }

    /// Mark the cache dirty (call after `vkCreateGraphicsPipelines`).
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Persist the blob to disk with an atomic write (temp file + rename).
    pub fn save(&mut self) -> Result<(), PipelineCacheError> {
        let dir = self.path.parent().unwrap_or_else(|| Path::new("."));
        let temp_path = dir.join(format!(
            "{}.tmp",
            self.path.file_name().unwrap_or_default().to_string_lossy()
        ));
        fs::write(&temp_path, &self.data)
            .map_err(|source| PipelineCacheError::io(&self.path, source))?;
        fs::rename(&temp_path, &self.path)
            .map_err(|source| PipelineCacheError::io(&self.path, source))?;
        self.dirty = false;
        Ok(())
    }
}

/// Append-only log of pipeline cache misses (Phase 51 infrastructure).
///
/// When a pipeline cache lookup misses at runtime, the miss is recorded here
/// so the compiler worker can prioritize missing pipelines on the next cycle.
/// The format is newline-delimited JSON, matching the `ProfileWriter` pattern.
pub struct PipelineMissLog {
    path: PathBuf,
    file: fs::File,
}

impl PipelineMissLog {
    /// Open or create a miss log at the given path.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, PipelineCacheError> {
        let path = path.into();
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent).map_err(|source| PipelineCacheError::io(&path, source))?;
        }
        let file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|source| PipelineCacheError::io(&path, source))?;
        Ok(Self { path, file })
    }

    /// Record a pipeline cache miss.
    pub fn record_miss(&mut self, pipeline_key: u64) -> Result<(), PipelineCacheError> {
        use io::Write;
        let line = format!("{{\"pipeline_key\":{pipeline_key}}}\n");
        self.file
            .write_all(line.as_bytes())
            .map_err(|source| PipelineCacheError::io(&self.path, source))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_open_creates_empty_when_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let blob = PipelineCacheBlob::open(dir.path()).expect("open");
        assert!(blob.data().is_empty());
        assert!(!blob.is_dirty());
    }

    #[test]
    fn blob_save_and_reload_round_trips() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut blob = PipelineCacheBlob::open(dir.path()).expect("open");
        blob.update(vec![1, 2, 3, 4]);
        assert!(blob.is_dirty());
        blob.save().expect("save");
        assert!(!blob.is_dirty());

        let reloaded = PipelineCacheBlob::open(dir.path()).expect("reload");
        assert_eq!(reloaded.data(), &[1, 2, 3, 4]);
        assert!(!reloaded.is_dirty());
    }

    #[test]
    fn miss_log_records_entries() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log_path = dir.path().join("misses.jsonl");
        let mut log = PipelineMissLog::open(&log_path).expect("open");
        log.record_miss(42).expect("record");
        log.record_miss(99).expect("record");

        let contents = fs::read_to_string(&log_path).expect("read");
        assert!(contents.contains("\"pipeline_key\":42"));
        assert!(contents.contains("\"pipeline_key\":99"));
    }
}
