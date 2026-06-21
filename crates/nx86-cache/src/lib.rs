//! CPU object cache (Phase 21).
//!
//! A [`CacheManager`] owns a directory of `.nxo` native objects (see
//! `nx86-object`) and provides the cache surface Continuous Dynamic Compilation
//! needs: a *manifest* of what is cached, integrity *checks* (a cheap
//! header-only "shallow" check and a hash-validating "full" check), size
//! accounting, and insert/load/remove/clear.
//!
//! A directory scan is always the source of truth; [`CacheManager::write_manifest`]
//! / [`CacheManager::read_manifest`] persist a `manifest.json` only as a fast
//! status snapshot, so a stale manifest can never misreport what is on disk.
//!
//! This crate is pure logic plus `std` file I/O, so it is host-independent and
//! fully testable on the development host.

use std::{
    fs::{self, File},
    io::{self, ErrorKind, Read, Write},
    path::{Path, PathBuf},
};

use nx86_object::{
    NativeObject, OBJECT_HEADER_LEN, OBJECT_VERSION, ObjectError, ObjectHeader, object_file_name,
};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use thiserror::Error;

pub const CRATE_NAME: &str = "nx86-cache";

#[must_use]
pub const fn crate_name() -> &'static str {
    CRATE_NAME
}

/// File name of the persisted manifest snapshot inside a cache directory.
pub const MANIFEST_FILE: &str = "manifest.json";

/// One cached native object, as seen without loading its code body.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CacheEntry {
    /// Guest entry PC this object was compiled for (its cache key).
    pub entry_address: u64,
    /// On-disk file name (`{entry:016x}.nxo`).
    pub file_name: String,
    /// Size of the object file in bytes.
    pub size_bytes: u64,
    /// `.nxo` format version recorded in the object header.
    pub version: u32,
}

/// A snapshot of every native object in a cache directory.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CacheManifest {
    pub entries: Vec<CacheEntry>,
}

impl CacheManifest {
    #[must_use]
    pub fn object_count(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn total_size_bytes(&self) -> u64 {
        self.entries.iter().map(|entry| entry.size_bytes).sum()
    }

    #[must_use]
    pub fn contains(&self, entry_address: u64) -> bool {
        self.get(entry_address).is_some()
    }

    #[must_use]
    pub fn get(&self, entry_address: u64) -> Option<&CacheEntry> {
        self.entries
            .iter()
            .find(|entry| entry.entry_address == entry_address)
    }

    #[must_use]
    pub fn status(&self) -> CacheStatus {
        CacheStatus {
            object_count: self.object_count(),
            total_bytes: self.total_size_bytes(),
        }
    }
}

/// Compact cache summary suitable for display (e.g. the GUI Library screen).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CacheStatus {
    pub object_count: usize,
    pub total_bytes: u64,
}

/// Result of integrity-checking a cached object at a given level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckOutcome {
    /// Present and valid at the requested level.
    Ok,
    /// No object is cached for this entry address.
    Missing,
    /// An object file exists but failed validation.
    Invalid,
}

/// Manages the native objects under a single cache directory.
#[derive(Clone, Debug)]
pub struct CacheManager {
    dir: PathBuf,
}

impl CacheManager {
    /// Open (creating if needed) the cache rooted at `dir`.
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self, CacheError> {
        let dir = dir.into();
        fs::create_dir_all(&dir).map_err(|source| CacheError::io(&dir, source))?;
        Ok(Self { dir })
    }

    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Path an object with `entry_address` would occupy in this cache.
    #[must_use]
    pub fn object_path(&self, entry_address: u64) -> PathBuf {
        self.dir.join(object_file_name(entry_address))
    }

    #[must_use]
    pub fn manifest_path(&self) -> PathBuf {
        self.dir.join(MANIFEST_FILE)
    }

    /// Scan the directory and build a fresh manifest. This is the source of
    /// truth; non-object and unreadable files are skipped.
    pub fn scan(&self) -> Result<CacheManifest, CacheError> {
        let read_dir = match fs::read_dir(&self.dir) {
            Ok(read_dir) => read_dir,
            Err(source) if source.kind() == ErrorKind::NotFound => {
                return Ok(CacheManifest::default());
            }
            Err(source) => return Err(CacheError::io(&self.dir, source)),
        };

        let mut entries = Vec::new();
        for entry in read_dir {
            let entry = entry.map_err(|source| CacheError::io(&self.dir, source))?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("nxo") {
                continue;
            }
            if !entry
                .file_type()
                .map_err(|source| CacheError::io(&path, source))?
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
            if file_name != object_file_name(header.entry_address) {
                continue;
            }
            let size_bytes = entry
                .metadata()
                .map_err(|source| CacheError::io(&path, source))?
                .len();
            entries.push(CacheEntry {
                entry_address: header.entry_address,
                file_name,
                size_bytes,
                version: header.version,
            });
        }
        entries.sort_by_key(|entry| entry.entry_address);
        Ok(CacheManifest { entries })
    }

    /// Convenience: scan and summarize for display.
    pub fn status(&self) -> Result<CacheStatus, CacheError> {
        Ok(self.scan()?.status())
    }

    /// Cheap check: the object file exists and its header magic, version, and
    /// entry address match. Does not validate the content hash.
    pub fn shallow_check(&self, entry_address: u64) -> Result<CheckOutcome, CacheError> {
        let path = self.object_path(entry_address);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if !metadata.file_type().is_file() => return Ok(CheckOutcome::Invalid),
            Ok(_) => {}
            Err(source) if source.kind() == ErrorKind::NotFound => {
                return Ok(CheckOutcome::Missing);
            }
            Err(source) => return Err(CacheError::io(&path, source)),
        }
        match header_of(&path)? {
            Some(header)
                if header.version == OBJECT_VERSION && header.entry_address == entry_address =>
            {
                Ok(CheckOutcome::Ok)
            }
            _ => Ok(CheckOutcome::Invalid),
        }
    }

    /// Full check: load the object and validate its content hash. This is the
    /// placeholder the SPEC §24 "full check" upgrades (e.g. executable-hash
    /// dependencies) later.
    pub fn full_check(&self, entry_address: u64) -> Result<CheckOutcome, CacheError> {
        let path = self.object_path(entry_address);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if !metadata.file_type().is_file() => return Ok(CheckOutcome::Invalid),
            Ok(_) => {}
            Err(source) if source.kind() == ErrorKind::NotFound => {
                return Ok(CheckOutcome::Missing);
            }
            Err(source) => return Err(CacheError::io(&path, source)),
        }
        match fs::read(&path) {
            Ok(bytes) => match NativeObject::from_bytes(&bytes) {
                Ok(object) if object.entry_address == entry_address => Ok(CheckOutcome::Ok),
                Ok(_) | Err(_) => Ok(CheckOutcome::Invalid),
            },
            Err(source) if source.kind() == ErrorKind::NotFound => Ok(CheckOutcome::Missing),
            Err(source) => Err(CacheError::io(&path, source)),
        }
    }

    /// Load and validate the cached object for `entry_address`.
    pub fn load(&self, entry_address: u64) -> Result<NativeObject, CacheError> {
        let path = self.object_path(entry_address);
        let metadata =
            fs::symlink_metadata(&path).map_err(|source| CacheError::io(&path, source))?;
        if !metadata.file_type().is_file() {
            return Err(CacheError::NotRegularObject { path });
        }
        let object = NativeObject::read_from_path(&path)?;
        if object.entry_address != entry_address {
            return Err(CacheError::EntryAddressMismatch {
                requested: entry_address,
                actual: object.entry_address,
            });
        }
        Ok(object)
    }

    /// Write `object` into the cache, returning its manifest entry.
    pub fn insert(&self, object: &NativeObject) -> Result<CacheEntry, CacheError> {
        let path = self.object_path(object.entry_address);
        write_atomic(&self.dir, &path, &object.to_bytes())?;
        let size_bytes = fs::metadata(&path)
            .map_err(|source| CacheError::io(&path, source))?
            .len();
        Ok(CacheEntry {
            entry_address: object.entry_address,
            file_name: object_file_name(object.entry_address),
            size_bytes,
            version: OBJECT_VERSION,
        })
    }

    /// Remove the object for `entry_address`. Returns whether a file was deleted.
    pub fn remove(&self, entry_address: u64) -> Result<bool, CacheError> {
        let path = self.object_path(entry_address);
        match fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(source) if source.kind() == ErrorKind::NotFound => Ok(false),
            Err(source) => Err(CacheError::io(&path, source)),
        }
    }

    /// Delete every cached object file. Returns the number of files removed.
    ///
    /// This removes every `.nxo` file by extension, not just objects the scan
    /// recognizes, so a corrupt or truncated object cannot survive a clear.
    pub fn clear(&self) -> Result<usize, CacheError> {
        let read_dir = match fs::read_dir(&self.dir) {
            Ok(read_dir) => read_dir,
            Err(source) if source.kind() == ErrorKind::NotFound => return Ok(0),
            Err(source) => return Err(CacheError::io(&self.dir, source)),
        };

        let mut removed = 0;
        for entry in read_dir {
            let entry = entry.map_err(|source| CacheError::io(&self.dir, source))?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("nxo") {
                continue;
            }
            if entry
                .file_type()
                .map_err(|source| CacheError::io(&path, source))?
                .is_dir()
            {
                continue;
            }
            match fs::remove_file(&path) {
                Ok(()) => removed += 1,
                Err(source) if source.kind() == ErrorKind::NotFound => {}
                Err(source) => return Err(CacheError::io(&path, source)),
            }
        }
        Ok(removed)
    }

    /// Persist a manifest snapshot to `manifest.json` for fast status reads.
    pub fn write_manifest(&self, manifest: &CacheManifest) -> Result<(), CacheError> {
        let json = serde_json::to_string_pretty(manifest).map_err(CacheError::Manifest)?;
        let path = self.manifest_path();
        write_atomic(&self.dir, &path, json.as_bytes())
    }

    /// Read the persisted manifest snapshot, if one exists. Prefer [`Self::scan`]
    /// when accuracy matters.
    pub fn read_manifest(&self) -> Result<Option<CacheManifest>, CacheError> {
        let path = self.manifest_path();
        match fs::symlink_metadata(&path) {
            Ok(metadata) if !metadata.file_type().is_file() => {
                return Err(CacheError::NotRegularManifest { path });
            }
            Ok(_) => {}
            Err(source) if source.kind() == ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(CacheError::io(&path, source)),
        }
        match fs::read_to_string(&path) {
            Ok(text) => {
                let manifest = serde_json::from_str(&text).map_err(CacheError::Manifest)?;
                Ok(Some(manifest))
            }
            // The file may be removed between metadata and read.
            Err(source) if source.kind() == ErrorKind::NotFound => Ok(None),
            Err(source) => Err(CacheError::io(&path, source)),
        }
    }
}

/// Read just the header of a `.nxo` file. Returns `None` if the file is absent,
/// shorter than a header, or not a valid object.
fn header_of(path: &Path) -> Result<Option<ObjectHeader>, CacheError> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(source) if source.kind() == ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(CacheError::io(path, source)),
    };
    let mut header = [0u8; OBJECT_HEADER_LEN];
    match file.read_exact(&mut header) {
        Ok(()) => {}
        Err(source) if source.kind() == ErrorKind::UnexpectedEof => return Ok(None),
        Err(source) => return Err(CacheError::io(path, source)),
    }
    Ok(NativeObject::read_header(&header).ok())
}

/// Atomically replace `path` with bytes written to a same-directory temporary
/// file. This avoids partial cache entries and never follows an existing
/// destination symlink.
fn write_atomic(dir: &Path, path: &Path, bytes: &[u8]) -> Result<(), CacheError> {
    let mut temporary = NamedTempFile::new_in(dir).map_err(|source| CacheError::io(dir, source))?;
    temporary
        .write_all(bytes)
        .map_err(|source| CacheError::io(temporary.path(), source))?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|source| CacheError::io(temporary.path(), source))?;
    temporary
        .persist(path)
        .map_err(|error| CacheError::io(path, error.error))?;
    Ok(())
}

/// A failure operating on the cache.
#[derive(Debug, Error)]
pub enum CacheError {
    #[error("cache I/O failed for {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error(transparent)]
    Object(#[from] ObjectError),
    #[error("cache manifest (de)serialization failed: {0}")]
    Manifest(serde_json::Error),
    #[error("cached object entry mismatch: requested {requested:#x}, object contains {actual:#x}")]
    EntryAddressMismatch { requested: u64, actual: u64 },
    #[error("cached object path is not a regular file: {path}")]
    NotRegularObject { path: PathBuf },
    #[error("cache manifest path is not a regular file: {path}")]
    NotRegularManifest { path: PathBuf },
}

impl CacheError {
    fn io(path: &Path, source: io::Error) -> Self {
        Self::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

#[cfg(test)]
mod tests {
    use nx86_object::NativeObject;
    use tempfile::tempdir;

    use super::{CacheError, CacheManager, CheckOutcome};

    fn object(entry_address: u64, code: Vec<u8>) -> NativeObject {
        NativeObject {
            entry_address,
            guest_end: entry_address + 4,
            stack_size: 0,
            code,
        }
    }

    #[test]
    fn empty_cache_scans_clean() {
        let dir = tempdir().expect("temp dir");
        let cache = CacheManager::open(dir.path()).expect("open cache");
        let status = cache.status().expect("status");
        assert_eq!(status.object_count, 0);
        assert_eq!(status.total_bytes, 0);
    }

    #[test]
    fn insert_load_and_account() {
        let dir = tempdir().expect("temp dir");
        let cache = CacheManager::open(dir.path()).expect("open cache");

        let a = object(0x4000, vec![0x90, 0xC3]);
        let b = object(0x8000, vec![0xC3]);
        let entry_a = cache.insert(&a).expect("insert a");
        let entry_b = cache.insert(&b).expect("insert b");

        let manifest = cache.scan().expect("scan");
        assert_eq!(manifest.object_count(), 2);
        // Entries are sorted by entry address.
        assert_eq!(manifest.entries[0].entry_address, 0x4000);
        assert_eq!(manifest.entries[1].entry_address, 0x8000);
        assert!(manifest.contains(0x4000));
        assert_eq!(
            manifest.total_size_bytes(),
            entry_a.size_bytes + entry_b.size_bytes
        );

        let loaded = cache.load(0x4000).expect("load a");
        assert_eq!(loaded, a);
    }

    #[test]
    fn load_rejects_object_stored_under_the_wrong_key() {
        let dir = tempdir().expect("temp dir");
        let cache = CacheManager::open(dir.path()).expect("open cache");
        let wrong_object = object(0x2000, vec![0xc3]);
        wrong_object
            .write_to_path(&cache.object_path(0x1000))
            .expect("write mismatched object");

        assert!(matches!(
            cache.load(0x1000),
            Err(CacheError::EntryAddressMismatch {
                requested: 0x1000,
                actual: 0x2000
            })
        ));
        assert_eq!(cache.scan().expect("scan").object_count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn insert_replaces_symlink_without_overwriting_its_target() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().expect("temp dir");
        let cache = CacheManager::open(dir.path()).expect("open cache");
        let outside = dir.path().join("outside.bin");
        std::fs::write(&outside, b"outside data").expect("write outside target");
        let object_path = cache.object_path(0x1000);
        symlink(&outside, &object_path).expect("create cache symlink");

        assert_eq!(
            cache.shallow_check(0x1000).expect("shallow check"),
            CheckOutcome::Invalid
        );
        assert_eq!(
            cache.full_check(0x1000).expect("full check"),
            CheckOutcome::Invalid
        );
        assert!(matches!(
            cache.load(0x1000),
            Err(CacheError::NotRegularObject { .. })
        ));

        let object = object(0x1000, vec![0xc3]);
        cache.insert(&object).expect("insert object");

        assert_eq!(
            std::fs::read(&outside).expect("read outside target"),
            b"outside data"
        );
        assert_eq!(cache.load(0x1000).expect("load object"), object);
    }

    #[test]
    fn shallow_passes_where_full_fails_on_corruption() {
        let dir = tempdir().expect("temp dir");
        let cache = CacheManager::open(dir.path()).expect("open cache");
        let obj = object(0x1000, vec![0x90, 0x90, 0xC3]);
        cache.insert(&obj).expect("insert");

        // Corrupt a code byte: header stays intact, content hash no longer matches.
        let path = cache.object_path(0x1000);
        let mut bytes = std::fs::read(&path).expect("read object");
        bytes[nx86_object::OBJECT_HEADER_LEN] ^= 0xFF;
        std::fs::write(&path, &bytes).expect("write corrupt object");

        assert_eq!(
            cache.shallow_check(0x1000).expect("shallow"),
            CheckOutcome::Ok
        );
        assert_eq!(
            cache.full_check(0x1000).expect("full"),
            CheckOutcome::Invalid
        );
    }

    #[test]
    fn checks_report_missing() {
        let dir = tempdir().expect("temp dir");
        let cache = CacheManager::open(dir.path()).expect("open cache");
        assert_eq!(
            cache.shallow_check(0x10).expect("shallow"),
            CheckOutcome::Missing
        );
        assert_eq!(cache.full_check(0x10).expect("full"), CheckOutcome::Missing);
    }

    #[test]
    fn remove_and_clear() {
        let dir = tempdir().expect("temp dir");
        let cache = CacheManager::open(dir.path()).expect("open cache");
        cache.insert(&object(0x1, vec![0xC3])).expect("insert 1");
        cache.insert(&object(0x2, vec![0xC3])).expect("insert 2");
        cache.insert(&object(0x3, vec![0xC3])).expect("insert 3");

        assert!(cache.remove(0x2).expect("remove existing"));
        assert!(!cache.remove(0x2).expect("remove missing"));
        assert_eq!(cache.scan().expect("scan").object_count(), 2);

        assert_eq!(cache.clear().expect("clear"), 2);
        assert_eq!(cache.scan().expect("scan").object_count(), 0);
    }

    #[test]
    fn clear_removes_corrupt_objects() {
        let dir = tempdir().expect("temp dir");
        let cache = CacheManager::open(dir.path()).expect("open cache");
        cache.insert(&object(0x1, vec![0xC3])).expect("insert");
        // A corrupt object the scan does not recognize must still be cleared.
        std::fs::write(dir.path().join("000000000000beef.nxo"), b"not an object")
            .expect("write corrupt object");
        std::fs::create_dir(dir.path().join("not-an-object.nxo")).expect("create directory");

        // scan only sees the one valid object, but clear removes both files.
        assert_eq!(cache.scan().expect("scan").object_count(), 1);
        assert_eq!(cache.clear().expect("clear"), 2);
        assert!(dir.path().join("not-an-object.nxo").is_dir());
        assert!(!cache.object_path(0x1).exists());
        assert!(!dir.path().join("000000000000beef.nxo").exists());
    }

    #[test]
    fn manifest_round_trips_and_is_not_an_object() {
        let dir = tempdir().expect("temp dir");
        let cache = CacheManager::open(dir.path()).expect("open cache");
        cache.insert(&object(0x20, vec![0xC3])).expect("insert");

        let manifest = cache.scan().expect("scan");
        cache.write_manifest(&manifest).expect("write manifest");

        // The persisted manifest.json must not be mistaken for a cached object.
        let rescanned = cache.scan().expect("rescan");
        assert_eq!(rescanned.object_count(), 1);

        let restored = cache
            .read_manifest()
            .expect("read manifest")
            .expect("present");
        assert_eq!(restored, manifest);
    }

    #[cfg(unix)]
    #[test]
    fn manifest_write_replaces_symlink_without_overwriting_its_target() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().expect("temp dir");
        let cache = CacheManager::open(dir.path()).expect("open cache");
        let outside = dir.path().join("outside-manifest.json");
        std::fs::write(&outside, b"outside data").expect("write outside target");
        symlink(&outside, cache.manifest_path()).expect("create manifest symlink");
        let manifest = super::CacheManifest::default();

        assert!(matches!(
            cache.read_manifest(),
            Err(CacheError::NotRegularManifest { .. })
        ));
        cache.write_manifest(&manifest).expect("write manifest");

        assert_eq!(
            std::fs::read(&outside).expect("read outside target"),
            b"outside data"
        );
        assert_eq!(
            cache.read_manifest().expect("read manifest"),
            Some(manifest)
        );
    }
}
