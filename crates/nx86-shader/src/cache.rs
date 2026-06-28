//! Per-title shader object cache.
//!
//! A [`ShaderCache`] owns a directory of `.nxshader` objects (typically a
//! title's `cache/shaders/` folder) and provides the same surface the CPU cache
//! does in `nx86-cache`: a manifest, shallow/full integrity checks, size
//! accounting, and insert/load/remove/clear. A directory scan is always the
//! source of truth; the persisted `shader-manifest.json` is only a fast status
//! snapshot. Pure logic plus `std` file I/O, so it is host-independent.

use std::{
    fs::{self, File},
    io::{self, ErrorKind, Read, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use thiserror::Error;

use crate::object::{
    SHADER_OBJECT_HEADER_LEN, SHADER_OBJECT_VERSION, ShaderObject, ShaderObjectError,
    ShaderObjectHeader, shader_object_file_name,
};
use crate::{ShaderHash, ShaderStage};

/// File name of the persisted manifest snapshot inside a shader cache directory.
pub const SHADER_MANIFEST_FILE: &str = "shader-manifest.json";

const OBJECT_EXTENSION: &str = "nxshader";

/// One cached shader object, as seen without loading its translated body.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct ShaderCacheEntry {
    /// Source hash this object was translated from (its cache key).
    pub source_hash: ShaderHash,
    /// On-disk file name (`{source_hash:016x}.nxshader`).
    pub file_name: String,
    /// Size of the object file in bytes.
    pub size_bytes: u64,
    /// Pipeline stage recorded in the object header.
    pub stage: ShaderStage,
    /// `.nxshader` format version recorded in the object header.
    pub version: u32,
}

/// A snapshot of every shader object in a cache directory.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct ShaderCacheManifest {
    pub entries: Vec<ShaderCacheEntry>,
}

impl ShaderCacheManifest {
    #[must_use]
    pub fn object_count(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn total_size_bytes(&self) -> u64 {
        self.entries.iter().map(|entry| entry.size_bytes).sum()
    }

    #[must_use]
    pub fn contains(&self, source_hash: ShaderHash) -> bool {
        self.get(source_hash).is_some()
    }

    #[must_use]
    pub fn get(&self, source_hash: ShaderHash) -> Option<&ShaderCacheEntry> {
        self.entries
            .iter()
            .find(|entry| entry.source_hash == source_hash)
    }

    #[must_use]
    pub fn status(&self) -> ShaderCacheStatus {
        ShaderCacheStatus {
            object_count: self.object_count(),
            total_bytes: self.total_size_bytes(),
        }
    }
}

/// Compact shader-cache summary suitable for display.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct ShaderCacheStatus {
    pub object_count: usize,
    pub total_bytes: u64,
}

/// Result of integrity-checking a cached shader object at a given level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShaderCheckOutcome {
    Ok,
    Missing,
    Invalid,
}

/// Manages the shader objects under a single cache directory.
#[derive(Clone, Debug)]
pub struct ShaderCache {
    dir: PathBuf,
}

impl ShaderCache {
    /// Open (creating if needed) the shader cache rooted at `dir`.
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self, ShaderCacheError> {
        let dir = dir.into();
        fs::create_dir_all(&dir).map_err(|source| ShaderCacheError::io(&dir, source))?;
        Ok(Self { dir })
    }

    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Path an object with `source_hash` would occupy in this cache.
    #[must_use]
    pub fn object_path(&self, source_hash: ShaderHash) -> PathBuf {
        self.dir.join(shader_object_file_name(source_hash))
    }

    #[must_use]
    pub fn manifest_path(&self) -> PathBuf {
        self.dir.join(SHADER_MANIFEST_FILE)
    }

    /// Scan the directory and build a fresh manifest. This is the source of
    /// truth; non-object and unreadable files are skipped.
    pub fn scan(&self) -> Result<ShaderCacheManifest, ShaderCacheError> {
        let read_dir = match fs::read_dir(&self.dir) {
            Ok(read_dir) => read_dir,
            Err(source) if source.kind() == ErrorKind::NotFound => {
                return Ok(ShaderCacheManifest::default());
            }
            Err(source) => return Err(ShaderCacheError::io(&self.dir, source)),
        };

        let mut entries = Vec::new();
        for entry in read_dir {
            let entry = entry.map_err(|source| ShaderCacheError::io(&self.dir, source))?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some(OBJECT_EXTENSION) {
                continue;
            }
            if !entry
                .file_type()
                .map_err(|source| ShaderCacheError::io(&path, source))?
                .is_file()
            {
                continue;
            }
            let Some(header) = header_of(&path)? else {
                continue;
            };
            let file_name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_owned();
            if file_name != shader_object_file_name(header.source_hash) {
                continue;
            }
            let Ok(stage) = ShaderStage::from_code(header.stage_code) else {
                continue;
            };
            let size_bytes = entry
                .metadata()
                .map_err(|source| ShaderCacheError::io(&path, source))?
                .len();
            entries.push(ShaderCacheEntry {
                source_hash: header.source_hash,
                file_name,
                size_bytes,
                stage,
                version: header.version,
            });
        }
        entries.sort_by_key(|entry| entry.source_hash.as_u64());
        Ok(ShaderCacheManifest { entries })
    }

    /// Convenience: scan and summarize for display.
    pub fn status(&self) -> Result<ShaderCacheStatus, ShaderCacheError> {
        Ok(self.scan()?.status())
    }

    /// Cheap check: the object file exists and its header magic, version, and
    /// source hash match. Does not validate the content hash.
    pub fn shallow_check(
        &self,
        source_hash: ShaderHash,
    ) -> Result<ShaderCheckOutcome, ShaderCacheError> {
        let path = self.object_path(source_hash);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if !metadata.file_type().is_file() => {
                return Ok(ShaderCheckOutcome::Invalid);
            }
            Ok(_) => {}
            Err(source) if source.kind() == ErrorKind::NotFound => {
                return Ok(ShaderCheckOutcome::Missing);
            }
            Err(source) => return Err(ShaderCacheError::io(&path, source)),
        }
        match header_of(&path)? {
            Some(header)
                if header.version == SHADER_OBJECT_VERSION && header.source_hash == source_hash =>
            {
                Ok(ShaderCheckOutcome::Ok)
            }
            _ => Ok(ShaderCheckOutcome::Invalid),
        }
    }

    /// Full check: load the object and validate its content hash.
    pub fn full_check(
        &self,
        source_hash: ShaderHash,
    ) -> Result<ShaderCheckOutcome, ShaderCacheError> {
        let path = self.object_path(source_hash);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if !metadata.file_type().is_file() => {
                return Ok(ShaderCheckOutcome::Invalid);
            }
            Ok(_) => {}
            Err(source) if source.kind() == ErrorKind::NotFound => {
                return Ok(ShaderCheckOutcome::Missing);
            }
            Err(source) => return Err(ShaderCacheError::io(&path, source)),
        }
        match fs::read(&path) {
            Ok(bytes) => match ShaderObject::from_bytes(&bytes) {
                Ok(object) if object.source_hash == source_hash => Ok(ShaderCheckOutcome::Ok),
                Ok(_) | Err(_) => Ok(ShaderCheckOutcome::Invalid),
            },
            Err(source) if source.kind() == ErrorKind::NotFound => Ok(ShaderCheckOutcome::Missing),
            Err(source) => Err(ShaderCacheError::io(&path, source)),
        }
    }

    /// Load and validate the cached object for `source_hash`.
    pub fn load(&self, source_hash: ShaderHash) -> Result<ShaderObject, ShaderCacheError> {
        let path = self.object_path(source_hash);
        let metadata =
            fs::symlink_metadata(&path).map_err(|source| ShaderCacheError::io(&path, source))?;
        if !metadata.file_type().is_file() {
            return Err(ShaderCacheError::NotRegularObject { path });
        }
        let object = ShaderObject::read_from_path(&path)?;
        if object.source_hash != source_hash {
            return Err(ShaderCacheError::SourceHashMismatch {
                requested: source_hash.as_u64(),
                actual: object.source_hash.as_u64(),
            });
        }
        Ok(object)
    }

    /// Write `object` into the cache, returning its manifest entry.
    pub fn insert(&self, object: &ShaderObject) -> Result<ShaderCacheEntry, ShaderCacheError> {
        let path = self.object_path(object.source_hash);
        write_atomic(&self.dir, &path, &object.to_bytes())?;
        let size_bytes = fs::metadata(&path)
            .map_err(|source| ShaderCacheError::io(&path, source))?
            .len();
        Ok(ShaderCacheEntry {
            source_hash: object.source_hash,
            file_name: shader_object_file_name(object.source_hash),
            size_bytes,
            stage: object.stage,
            version: SHADER_OBJECT_VERSION,
        })
    }

    /// Remove the object for `source_hash`. Returns whether a file was deleted.
    pub fn remove(&self, source_hash: ShaderHash) -> Result<bool, ShaderCacheError> {
        let path = self.object_path(source_hash);
        match fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(source) if source.kind() == ErrorKind::NotFound => Ok(false),
            Err(source) => Err(ShaderCacheError::io(&path, source)),
        }
    }

    /// Delete every cached object file. Returns the number of files removed.
    pub fn clear(&self) -> Result<usize, ShaderCacheError> {
        let read_dir = match fs::read_dir(&self.dir) {
            Ok(read_dir) => read_dir,
            Err(source) if source.kind() == ErrorKind::NotFound => return Ok(0),
            Err(source) => return Err(ShaderCacheError::io(&self.dir, source)),
        };

        let mut removed = 0;
        for entry in read_dir {
            let entry = entry.map_err(|source| ShaderCacheError::io(&self.dir, source))?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some(OBJECT_EXTENSION) {
                continue;
            }
            if entry
                .file_type()
                .map_err(|source| ShaderCacheError::io(&path, source))?
                .is_dir()
            {
                continue;
            }
            match fs::remove_file(&path) {
                Ok(()) => removed += 1,
                Err(source) if source.kind() == ErrorKind::NotFound => {}
                Err(source) => return Err(ShaderCacheError::io(&path, source)),
            }
        }
        Ok(removed)
    }

    /// Persist a manifest snapshot for fast status reads.
    pub fn write_manifest(&self, manifest: &ShaderCacheManifest) -> Result<(), ShaderCacheError> {
        let json = serde_json::to_string_pretty(manifest).map_err(ShaderCacheError::Manifest)?;
        let path = self.manifest_path();
        write_atomic(&self.dir, &path, json.as_bytes())
    }

    /// Read the persisted manifest snapshot, if one exists. Prefer [`Self::scan`]
    /// when accuracy matters.
    pub fn read_manifest(&self) -> Result<Option<ShaderCacheManifest>, ShaderCacheError> {
        let path = self.manifest_path();
        match fs::symlink_metadata(&path) {
            Ok(metadata) if !metadata.file_type().is_file() => {
                return Err(ShaderCacheError::NotRegularManifest { path });
            }
            Ok(_) => {}
            Err(source) if source.kind() == ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(ShaderCacheError::io(&path, source)),
        }
        match fs::read_to_string(&path) {
            Ok(text) => {
                let manifest = serde_json::from_str(&text).map_err(ShaderCacheError::Manifest)?;
                Ok(Some(manifest))
            }
            Err(source) if source.kind() == ErrorKind::NotFound => Ok(None),
            Err(source) => Err(ShaderCacheError::io(&path, source)),
        }
    }
}

/// Read just the header of a `.nxshader` file. Returns `None` if the file is
/// absent, shorter than a header, or not a valid object.
fn header_of(path: &Path) -> Result<Option<ShaderObjectHeader>, ShaderCacheError> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(source) if source.kind() == ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(ShaderCacheError::io(path, source)),
    };
    let mut header = [0u8; SHADER_OBJECT_HEADER_LEN];
    match file.read_exact(&mut header) {
        Ok(()) => {}
        Err(source) if source.kind() == ErrorKind::UnexpectedEof => return Ok(None),
        Err(source) => return Err(ShaderCacheError::io(path, source)),
    }
    Ok(ShaderObject::read_header(&header).ok())
}

/// Atomically replace `path` with bytes written to a same-directory temporary
/// file, never following an existing destination symlink.
fn write_atomic(dir: &Path, path: &Path, bytes: &[u8]) -> Result<(), ShaderCacheError> {
    let mut temporary =
        NamedTempFile::new_in(dir).map_err(|source| ShaderCacheError::io(dir, source))?;
    temporary
        .write_all(bytes)
        .map_err(|source| ShaderCacheError::io(temporary.path(), source))?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|source| ShaderCacheError::io(temporary.path(), source))?;
    temporary
        .persist(path)
        .map_err(|error| ShaderCacheError::io(path, error.error))?;
    Ok(())
}

/// A failure operating on the shader cache.
#[derive(Debug, Error)]
pub enum ShaderCacheError {
    #[error("shader cache I/O failed for {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error(transparent)]
    Object(#[from] ShaderObjectError),
    #[error("shader cache manifest (de)serialization failed: {0}")]
    Manifest(serde_json::Error),
    #[error(
        "cached shader source mismatch: requested {requested:#018x}, object contains {actual:#018x}"
    )]
    SourceHashMismatch { requested: u64, actual: u64 },
    #[error("cached shader object path is not a regular file: {path}")]
    NotRegularObject { path: PathBuf },
    #[error("shader cache manifest path is not a regular file: {path}")]
    NotRegularManifest { path: PathBuf },
}

impl ShaderCacheError {
    fn io(path: &Path, source: io::Error) -> Self {
        Self::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::translate;

    fn object(stage: ShaderStage, source: &[u8]) -> ShaderObject {
        translate(stage, source, "main").to_object()
    }

    #[test]
    fn insert_load_round_trip_and_status() {
        let dir = tempdir().expect("temp dir");
        let cache = ShaderCache::open(dir.path()).expect("open");
        let object = object(ShaderStage::Vertex, b"void main() {}");

        let entry = cache.insert(&object).expect("insert");
        assert_eq!(entry.source_hash, object.source_hash);
        assert_eq!(entry.stage, ShaderStage::Vertex);

        let loaded = cache.load(object.source_hash).expect("load");
        assert_eq!(loaded, object);

        let status = cache.status().expect("status");
        assert_eq!(status.object_count, 1);
        assert_eq!(status.total_bytes, entry.size_bytes);
    }

    #[test]
    fn checks_report_presence_and_validity() {
        let dir = tempdir().expect("temp dir");
        let cache = ShaderCache::open(dir.path()).expect("open");
        let object = object(ShaderStage::Compute, b"compute");
        let missing = ShaderHash(0xdead_beef);

        assert_eq!(
            cache.shallow_check(missing).expect("shallow"),
            ShaderCheckOutcome::Missing
        );
        cache.insert(&object).expect("insert");
        assert_eq!(
            cache.shallow_check(object.source_hash).expect("shallow"),
            ShaderCheckOutcome::Ok
        );
        assert_eq!(
            cache.full_check(object.source_hash).expect("full"),
            ShaderCheckOutcome::Ok
        );

        // Corrupt the body: shallow still passes (header intact), full fails.
        let path = cache.object_path(object.source_hash);
        let mut bytes = fs::read(&path).expect("read");
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        fs::write(&path, &bytes).expect("write");
        assert_eq!(
            cache.full_check(object.source_hash).expect("full"),
            ShaderCheckOutcome::Invalid
        );
    }

    #[test]
    fn scan_is_source_of_truth_and_clear_empties() {
        let dir = tempdir().expect("temp dir");
        let cache = ShaderCache::open(dir.path()).expect("open");
        cache.insert(&object(ShaderStage::Vertex, b"a")).expect("a");
        cache
            .insert(&object(ShaderStage::Fragment, b"b"))
            .expect("b");

        let manifest = cache.scan().expect("scan");
        assert_eq!(manifest.object_count(), 2);
        cache.write_manifest(&manifest).expect("write manifest");
        assert_eq!(
            cache.read_manifest().expect("read").as_ref(),
            Some(&manifest)
        );

        assert_eq!(cache.clear().expect("clear"), 2);
        assert_eq!(cache.scan().expect("rescan").object_count(), 0);
    }

    #[test]
    fn remove_reports_whether_a_file_was_deleted() {
        let dir = tempdir().expect("temp dir");
        let cache = ShaderCache::open(dir.path()).expect("open");
        let object = object(ShaderStage::Vertex, b"x");
        assert!(!cache.remove(object.source_hash).expect("remove missing"));
        cache.insert(&object).expect("insert");
        assert!(cache.remove(object.source_hash).expect("remove present"));
    }
}
