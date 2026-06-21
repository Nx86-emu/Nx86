//! Versioned append-only runtime profiles (Phase 24).
//!
//! Runtime observations are persisted as newline-delimited JSON. Each line is
//! independently versioned and contains one typed event, allowing Phase 25 to
//! consume complete records even when a crash leaves the final line truncated.

use std::{
    collections::HashSet,
    fmt::Debug,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const CRATE_NAME: &str = "nx86-profile";
/// On-disk runtime profile format emitted by this crate.
pub const PROFILE_FORMAT_VERSION: u32 = 1;
/// Maximum serialized size of one profile record accepted by the reader.
pub const MAX_PROFILE_RECORD_BYTES: usize = 16 * 1024;

#[must_use]
pub const fn crate_name() -> &'static str {
    CRATE_NAME
}

/// One versioned line in a runtime profile.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProfileRecord {
    pub format_version: u32,
    #[serde(flatten)]
    pub event: ProfileEvent,
}

impl ProfileRecord {
    #[must_use]
    pub const fn new(event: ProfileEvent) -> Self {
        Self {
            format_version: PROFILE_FORMAT_VERSION,
            event,
        }
    }
}

/// Runtime observation persisted for later profile-guided compilation.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProfileEvent {
    JitBlock {
        guest_pc: u64,
        code_size_bytes: u64,
        cache_file_name: String,
    },
    BranchTarget {
        source_pc: u64,
        target_pc: u64,
    },
    HelperCall {
        guest_pc: u64,
        helper_id: String,
    },
    Slowmem {
        guest_pc: u64,
        address: u64,
        size_bytes: u32,
        access: MemoryAccessKind,
        reason_code: String,
    },
}

/// Kind of guest-memory access that required a slow path.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryAccessKind {
    Read,
    Write,
    Execute,
}

/// Whether a profile event was appended or suppressed by file-wide
/// deduplication.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordOutcome {
    Written,
    Duplicate,
}

/// A destination for typed runtime profile events.
pub trait ProfileSink: Debug {
    fn record(&mut self, event: ProfileEvent) -> Result<RecordOutcome, ProfileError>;
}

/// Append-only writer for one runtime profile file.
#[derive(Debug)]
pub struct ProfileWriter {
    path: PathBuf,
    file: File,
    valid_len: u64,
    branch_targets: HashSet<(u64, u64)>,
}

impl ProfileWriter {
    /// Open or create a profile, repairing an incomplete final record before
    /// future appends and loading branch pairs for file-wide deduplication.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, ProfileError> {
        let path = path.into();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent).map_err(|source| ProfileError::io(parent, source))?;
        }

        match fs::symlink_metadata(&path) {
            Ok(metadata) if !metadata.file_type().is_file() => {
                return Err(ProfileError::NotRegularFile { path });
            }
            Ok(_) | Err(_) => {}
        }

        let mut options = OpenOptions::new();
        options.create(true).read(true).append(true);
        set_no_follow(&mut options);
        let mut file = options
            .open(&path)
            .map_err(|source| ProfileError::io(&path, source))?;
        if !file
            .metadata()
            .map_err(|source| ProfileError::io(&path, source))?
            .file_type()
            .is_file()
        {
            return Err(ProfileError::NotRegularFile { path });
        }
        lock_exclusive(&file, &path)?;

        file.seek(SeekFrom::Start(0))
            .map_err(|source| ProfileError::io(&path, source))?;
        let mut existing = Vec::new();
        file.read_to_end(&mut existing)
            .map_err(|source| ProfileError::io(&path, source))?;

        let parsed = parse_profile(&existing)?;
        let valid_len = match parsed.tail_repair {
            TailRepair::None => existing.len() as u64,
            TailRepair::AddNewline => {
                file.write_all(b"\n")
                    .map_err(|source| ProfileError::io(&path, source))?;
                existing.len() as u64 + 1
            }
            TailRepair::Truncate(valid_len) => {
                file.set_len(valid_len as u64)
                    .map_err(|source| ProfileError::io(&path, source))?;
                valid_len as u64
            }
        };

        let branch_targets = parsed
            .log
            .records
            .iter()
            .filter_map(|record| match &record.event {
                ProfileEvent::BranchTarget {
                    source_pc,
                    target_pc,
                } => Some((*source_pc, *target_pc)),
                ProfileEvent::JitBlock { .. }
                | ProfileEvent::HelperCall { .. }
                | ProfileEvent::Slowmem { .. } => None,
            })
            .collect();

        Ok(Self {
            path,
            file,
            valid_len,
            branch_targets,
        })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl ProfileSink for ProfileWriter {
    fn record(&mut self, event: ProfileEvent) -> Result<RecordOutcome, ProfileError> {
        validate_event(&event)?;
        let branch_target = match &event {
            ProfileEvent::BranchTarget {
                source_pc,
                target_pc,
            } => Some((*source_pc, *target_pc)),
            ProfileEvent::JitBlock { .. }
            | ProfileEvent::HelperCall { .. }
            | ProfileEvent::Slowmem { .. } => None,
        };
        if branch_target.is_some_and(|target| self.branch_targets.contains(&target)) {
            return Ok(RecordOutcome::Duplicate);
        }

        let mut bytes =
            serde_json::to_vec(&ProfileRecord::new(event)).map_err(ProfileError::Serialize)?;
        bytes.push(b'\n');
        let next_valid_len = self
            .valid_len
            .checked_add(bytes.len() as u64)
            .ok_or(ProfileError::FileTooLarge)?;
        if let Err(write_error) = self.file.write_all(&bytes) {
            return match self.file.set_len(self.valid_len) {
                Ok(()) => Err(ProfileError::io(&self.path, write_error)),
                Err(rollback_error) => Err(ProfileError::WriteRollback {
                    path: self.path.clone(),
                    write_error,
                    rollback_error,
                }),
            };
        }
        self.valid_len = next_valid_len;
        if let Some(target) = branch_target {
            self.branch_targets.insert(target);
        }
        Ok(RecordOutcome::Written)
    }
}

/// Parsed profile records and crash-tail recovery status.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProfileLog {
    pub records: Vec<ProfileRecord>,
    pub recovered_truncated_tail: bool,
}

impl ProfileLog {
    /// Extract `JitBlock` events as rebuild candidates, deduplicated by
    /// `guest_pc` (first occurrence wins). Each candidate carries the
    /// `guest_pc` needed to recompile the block through the AOT pipeline.
    #[must_use]
    pub fn jit_block_candidates(&self) -> Vec<JitBlockCandidate<'_>> {
        let mut seen = HashSet::new();
        self.records
            .iter()
            .filter_map(|record| match &record.event {
                ProfileEvent::JitBlock {
                    guest_pc,
                    code_size_bytes,
                    cache_file_name,
                } => {
                    if seen.insert(*guest_pc) {
                        Some(JitBlockCandidate {
                            guest_pc: *guest_pc,
                            code_size_bytes: *code_size_bytes,
                            cache_file_name,
                        })
                    } else {
                        None
                    }
                }
                ProfileEvent::BranchTarget { .. }
                | ProfileEvent::HelperCall { .. }
                | ProfileEvent::Slowmem { .. } => None,
            })
            .collect()
    }
}

/// A JIT-compiled block identified from a runtime profile as a candidate for
/// AOT promotion. Borrows string data from the profile record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JitBlockCandidate<'a> {
    pub guest_pc: u64,
    pub code_size_bytes: u64,
    pub cache_file_name: &'a str,
}

/// Read all complete records from a profile. An invalid unterminated final line
/// is treated as a crash-truncated tail; malformed complete lines are errors.
pub fn read_profile(path: impl AsRef<Path>) -> Result<ProfileLog, ProfileError> {
    let path = path.as_ref();
    let metadata = fs::symlink_metadata(path).map_err(|source| ProfileError::io(path, source))?;
    if !metadata.file_type().is_file() {
        return Err(ProfileError::NotRegularFile {
            path: path.to_path_buf(),
        });
    }
    let mut options = OpenOptions::new();
    options.read(true);
    set_no_follow(&mut options);
    let mut file = options
        .open(path)
        .map_err(|source| ProfileError::io(path, source))?;
    if !file
        .metadata()
        .map_err(|source| ProfileError::io(path, source))?
        .file_type()
        .is_file()
    {
        return Err(ProfileError::NotRegularFile {
            path: path.to_path_buf(),
        });
    }
    lock_shared(&file, path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|source| ProfileError::io(path, source))?;
    Ok(parse_profile(&bytes)?.log)
}

#[derive(Debug)]
struct ParsedProfile {
    log: ProfileLog,
    tail_repair: TailRepair,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TailRepair {
    None,
    AddNewline,
    Truncate(usize),
}

fn parse_profile(bytes: &[u8]) -> Result<ParsedProfile, ProfileError> {
    if bytes.is_empty() {
        return Ok(ParsedProfile {
            log: ProfileLog::default(),
            tail_repair: TailRepair::None,
        });
    }

    let terminated = bytes.last() == Some(&b'\n');
    let mut records = Vec::new();
    let mut line_start = 0;
    let mut line_number = 1;

    while line_start < bytes.len() {
        let line_end = bytes[line_start..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(bytes.len(), |offset| line_start + offset);
        let line = &bytes[line_start..line_end];
        let is_unterminated_tail = line_end == bytes.len() && !terminated;

        if line.is_empty() {
            return Err(ProfileError::EmptyRecord { line: line_number });
        }
        if line.len() > MAX_PROFILE_RECORD_BYTES {
            return Err(ProfileError::RecordTooLarge {
                line: line_number,
                size_bytes: line.len(),
            });
        }

        let value = match serde_json::from_slice::<serde_json::Value>(line) {
            Ok(value) => value,
            Err(_) if is_unterminated_tail => {
                return Ok(ParsedProfile {
                    log: ProfileLog {
                        records,
                        recovered_truncated_tail: true,
                    },
                    tail_repair: TailRepair::Truncate(line_start),
                });
            }
            Err(source) => {
                return Err(ProfileError::MalformedRecord {
                    line: line_number,
                    source,
                });
            }
        };
        let record = serde_json::from_slice::<ProfileRecord>(line).map_err(|source| {
            ProfileError::MalformedRecord {
                line: line_number,
                source,
            }
        })?;
        validate_version(&record, line_number)?;
        validate_record_shape(&value, &record.event, line_number)?;
        validate_event(&record.event)?;
        records.push(record);

        if line_end == bytes.len() {
            break;
        }
        line_start = line_end + 1;
        line_number += 1;
    }

    Ok(ParsedProfile {
        log: ProfileLog {
            records,
            recovered_truncated_tail: false,
        },
        tail_repair: if terminated {
            TailRepair::None
        } else {
            TailRepair::AddNewline
        },
    })
}

fn validate_version(record: &ProfileRecord, line: usize) -> Result<(), ProfileError> {
    if record.format_version == PROFILE_FORMAT_VERSION {
        Ok(())
    } else {
        Err(ProfileError::UnsupportedVersion {
            line,
            found: record.format_version,
        })
    }
}

fn validate_record_shape(
    value: &serde_json::Value,
    event: &ProfileEvent,
    line: usize,
) -> Result<(), ProfileError> {
    let object = value
        .as_object()
        .ok_or(ProfileError::UnexpectedFields { line })?;
    let expected: &[&str] = match event {
        ProfileEvent::JitBlock { .. } => &[
            "format_version",
            "kind",
            "guest_pc",
            "code_size_bytes",
            "cache_file_name",
        ],
        ProfileEvent::BranchTarget { .. } => &["format_version", "kind", "source_pc", "target_pc"],
        ProfileEvent::HelperCall { .. } => &["format_version", "kind", "guest_pc", "helper_id"],
        ProfileEvent::Slowmem { .. } => &[
            "format_version",
            "kind",
            "guest_pc",
            "address",
            "size_bytes",
            "access",
            "reason_code",
        ],
    };
    if object.len() != expected.len()
        || object
            .keys()
            .any(|field| !expected.contains(&field.as_str()))
    {
        return Err(ProfileError::UnexpectedFields { line });
    }
    Ok(())
}

fn validate_event(event: &ProfileEvent) -> Result<(), ProfileError> {
    match event {
        ProfileEvent::JitBlock {
            cache_file_name, ..
        } if !is_object_cache_name(cache_file_name) => Err(ProfileError::InvalidField {
            field: "cache_file_name",
        }),
        ProfileEvent::JitBlock {
            code_size_bytes: 0, ..
        } => Err(ProfileError::InvalidField {
            field: "code_size_bytes",
        }),
        ProfileEvent::HelperCall { helper_id, .. } if !is_identifier(helper_id) => {
            Err(ProfileError::InvalidField { field: "helper_id" })
        }
        ProfileEvent::Slowmem { reason_code, .. } if !is_identifier(reason_code) => {
            Err(ProfileError::InvalidField {
                field: "reason_code",
            })
        }
        ProfileEvent::Slowmem { size_bytes, .. } if !matches!(*size_bytes, 1 | 2 | 4 | 8 | 16) => {
            Err(ProfileError::InvalidField {
                field: "size_bytes",
            })
        }
        ProfileEvent::JitBlock { .. }
        | ProfileEvent::BranchTarget { .. }
        | ProfileEvent::HelperCall { .. }
        | ProfileEvent::Slowmem { .. } => Ok(()),
    }
}

fn is_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

fn is_object_cache_name(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 20
        && bytes[16..] == *b".nxo"
        && bytes[..16]
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
}

#[cfg(unix)]
fn set_no_follow(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;

    options.custom_flags(libc::O_NOFOLLOW);
}

#[cfg(not(unix))]
fn set_no_follow(_options: &mut OpenOptions) {}

#[cfg(unix)]
fn lock_exclusive(file: &File, path: &Path) -> Result<(), ProfileError> {
    lock_file(file, path, libc::LOCK_EX)
}

#[cfg(not(unix))]
fn lock_exclusive(_file: &File, _path: &Path) -> Result<(), ProfileError> {
    Ok(())
}

#[cfg(unix)]
fn lock_shared(file: &File, path: &Path) -> Result<(), ProfileError> {
    lock_file(file, path, libc::LOCK_SH)
}

#[cfg(not(unix))]
fn lock_shared(_file: &File, _path: &Path) -> Result<(), ProfileError> {
    Ok(())
}

#[cfg(unix)]
#[allow(unsafe_code)]
fn lock_file(file: &File, path: &Path, operation: libc::c_int) -> Result<(), ProfileError> {
    use std::os::fd::AsRawFd;

    // SAFETY: `file` owns a live descriptor for the duration of this call.
    let result = unsafe { libc::flock(file.as_raw_fd(), operation | libc::LOCK_NB) };
    if result == 0 {
        return Ok(());
    }
    let source = io::Error::last_os_error();
    if source.kind() == io::ErrorKind::WouldBlock {
        Err(ProfileError::AlreadyLocked {
            path: path.to_path_buf(),
        })
    } else {
        Err(ProfileError::io(path, source))
    }
}

/// A failure reading or writing a runtime profile.
#[derive(Debug, Error)]
pub enum ProfileError {
    #[error("profile I/O failed for {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("profile path is not a regular file: {path}")]
    NotRegularFile { path: PathBuf },
    #[error("profile file is already open for writing: {path}")]
    AlreadyLocked { path: PathBuf },
    #[error("profile file length exceeds the supported range")]
    FileTooLarge,
    #[error("profile record serialization failed: {0}")]
    Serialize(serde_json::Error),
    #[error("profile record on line {line} is malformed: {source}")]
    MalformedRecord {
        line: usize,
        source: serde_json::Error,
    },
    #[error("profile record on line {line} uses unsupported format version {found}")]
    UnsupportedVersion { line: usize, found: u32 },
    #[error("profile record on line {line} is empty")]
    EmptyRecord { line: usize },
    #[error("profile record on line {line} is too large ({size_bytes} bytes)")]
    RecordTooLarge { line: usize, size_bytes: usize },
    #[error("profile record on line {line} contains unexpected fields")]
    UnexpectedFields { line: usize },
    #[error("profile field `{field}` is not a safe deterministic identifier")]
    InvalidField { field: &'static str },
    #[error(
        "profile write failed for {path}: {write_error}; rollback also failed: {rollback_error}"
    )]
    WriteRollback {
        path: PathBuf,
        write_error: io::Error,
        rollback_error: io::Error,
    },
}

impl ProfileError {
    fn io(path: &Path, source: io::Error) -> Self {
        Self::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{
        JitBlockCandidate, MemoryAccessKind, ProfileError, ProfileEvent, ProfileLog, ProfileRecord,
        ProfileSink, ProfileWriter, RecordOutcome, read_profile,
    };

    fn all_events() -> Vec<ProfileEvent> {
        vec![
            ProfileEvent::JitBlock {
                guest_pc: 0x1000,
                code_size_bytes: 42,
                cache_file_name: "0000000000001000.nxo".to_owned(),
            },
            ProfileEvent::BranchTarget {
                source_pc: 0x1000,
                target_pc: 0x2000,
            },
            ProfileEvent::HelperCall {
                guest_pc: 0x2000,
                helper_id: "svc.dispatch".to_owned(),
            },
            ProfileEvent::Slowmem {
                guest_pc: 0x2004,
                address: 0x8000,
                size_bytes: 8,
                access: MemoryAccessKind::Read,
                reason_code: "page-not-fastmem".to_owned(),
            },
        ]
    }

    #[test]
    fn round_trips_every_event_type() {
        let dir = tempdir().expect("temp dir");
        let path = dir.path().join("runtime-v1.jsonl");
        let mut writer = ProfileWriter::open(&path).expect("open profile");
        for event in all_events() {
            assert_eq!(
                writer.record(event).expect("record event"),
                RecordOutcome::Written
            );
        }
        drop(writer);

        let log = read_profile(&path).expect("read profile");
        assert!(!log.recovered_truncated_tail);
        assert_eq!(
            log.records,
            all_events()
                .into_iter()
                .map(ProfileRecord::new)
                .collect::<Vec<_>>()
        );
        let text = fs::read_to_string(path).expect("read text");
        assert!(text.contains("\"kind\":\"jit_block\""));
        assert!(text.contains("\"kind\":\"slowmem\""));
    }

    #[test]
    fn branch_pairs_are_unique_across_reopen() {
        let dir = tempdir().expect("temp dir");
        let path = dir.path().join("runtime-v1.jsonl");
        let branch = ProfileEvent::BranchTarget {
            source_pc: 0x10,
            target_pc: 0x20,
        };
        let mut writer = ProfileWriter::open(&path).expect("open profile");
        assert_eq!(
            writer.record(branch.clone()).expect("record branch"),
            RecordOutcome::Written
        );
        assert_eq!(
            writer.record(branch.clone()).expect("deduplicate branch"),
            RecordOutcome::Duplicate
        );
        drop(writer);

        let mut writer = ProfileWriter::open(&path).expect("reopen profile");
        assert_eq!(
            writer.record(branch).expect("deduplicate after reopen"),
            RecordOutcome::Duplicate
        );
        drop(writer);
        assert_eq!(read_profile(path).expect("read profile").records.len(), 1);
    }

    #[test]
    fn same_target_from_different_sources_is_not_a_duplicate() {
        let dir = tempdir().expect("temp dir");
        let path = dir.path().join("runtime-v1.jsonl");
        let mut writer = ProfileWriter::open(&path).expect("open profile");
        for source_pc in [0x10, 0x18] {
            assert_eq!(
                writer
                    .record(ProfileEvent::BranchTarget {
                        source_pc,
                        target_pc: 0x20,
                    })
                    .expect("record branch"),
                RecordOutcome::Written
            );
        }
        drop(writer);
        assert_eq!(read_profile(path).expect("read profile").records.len(), 2);
    }

    #[test]
    fn truncated_tail_is_reported_and_repaired_before_append() {
        let dir = tempdir().expect("temp dir");
        let path = dir.path().join("runtime-v1.jsonl");
        let complete = serde_json::to_string(&ProfileRecord::new(ProfileEvent::HelperCall {
            guest_pc: 1,
            helper_id: "first".to_owned(),
        }))
        .expect("serialize");
        fs::write(&path, format!("{complete}\n{{\"format_version\":1"))
            .expect("write truncated profile");

        let recovered = read_profile(&path).expect("recover profile");
        assert!(recovered.recovered_truncated_tail);
        assert_eq!(recovered.records.len(), 1);

        let mut writer = ProfileWriter::open(&path).expect("repair profile");
        writer
            .record(ProfileEvent::HelperCall {
                guest_pc: 2,
                helper_id: "second".to_owned(),
            })
            .expect("append after repair");
        drop(writer);
        let repaired = read_profile(path).expect("read repaired profile");
        assert!(!repaired.recovered_truncated_tail);
        assert_eq!(repaired.records.len(), 2);
    }

    #[test]
    fn valid_unterminated_record_gets_a_newline_before_append() {
        let dir = tempdir().expect("temp dir");
        let path = dir.path().join("runtime-v1.jsonl");
        let first = ProfileRecord::new(ProfileEvent::HelperCall {
            guest_pc: 1,
            helper_id: "first".to_owned(),
        });
        fs::write(&path, serde_json::to_vec(&first).expect("serialize")).expect("write profile");

        let mut writer = ProfileWriter::open(&path).expect("open profile");
        writer
            .record(ProfileEvent::HelperCall {
                guest_pc: 2,
                helper_id: "second".to_owned(),
            })
            .expect("append");
        drop(writer);
        assert_eq!(read_profile(path).expect("read profile").records.len(), 2);
    }

    #[test]
    fn malformed_complete_record_is_rejected() {
        let dir = tempdir().expect("temp dir");
        let path = dir.path().join("runtime-v1.jsonl");
        fs::write(&path, b"{not-json}\n").expect("write profile");
        assert!(matches!(
            ProfileWriter::open(path),
            Err(ProfileError::MalformedRecord { line: 1, .. })
        ));
    }

    #[test]
    fn unsupported_version_is_rejected() {
        let dir = tempdir().expect("temp dir");
        let path = dir.path().join("runtime-v1.jsonl");
        fs::write(
            &path,
            b"{\"format_version\":2,\"kind\":\"branch_target\",\"source_pc\":1,\"target_pc\":2}\n",
        )
        .expect("write profile");
        assert!(matches!(
            read_profile(path),
            Err(ProfileError::UnsupportedVersion { line: 1, found: 2 })
        ));
    }

    #[test]
    fn unexpected_fields_are_rejected() {
        let dir = tempdir().expect("temp dir");
        let path = dir.path().join("runtime-v1.jsonl");
        fs::write(
            &path,
            b"{\"format_version\":1,\"kind\":\"branch_target\",\"source_pc\":1,\"target_pc\":2,\"personal_path\":\"/Users/example\"}\n",
        )
        .expect("write profile");
        assert!(matches!(
            read_profile(path),
            Err(ProfileError::UnexpectedFields { line: 1 })
        ));
    }

    #[test]
    fn empty_oversized_and_duplicate_field_records_are_rejected() {
        let dir = tempdir().expect("temp dir");
        let path = dir.path().join("runtime-v1.jsonl");

        fs::write(&path, b"\n").expect("write empty record");
        assert!(matches!(
            read_profile(&path),
            Err(ProfileError::EmptyRecord { line: 1 })
        ));

        fs::write(&path, vec![b'x'; super::MAX_PROFILE_RECORD_BYTES + 1])
            .expect("write oversized record");
        assert!(matches!(
            read_profile(&path),
            Err(ProfileError::RecordTooLarge { line: 1, .. })
        ));

        fs::write(
            &path,
            b"{\"format_version\":1,\"kind\":\"branch_target\",\"source_pc\":1,\"source_pc\":2,\"target_pc\":3}\n",
        )
        .expect("write duplicate field");
        assert!(matches!(
            read_profile(path),
            Err(ProfileError::MalformedRecord { line: 1, .. })
        ));
    }

    #[test]
    fn complete_but_invalid_unterminated_record_is_not_discarded() {
        let dir = tempdir().expect("temp dir");
        let path = dir.path().join("runtime-v1.jsonl");
        fs::write(
            &path,
            b"{\"format_version\":1,\"kind\":\"branch_target\",\"source_pc\":\"bad\",\"target_pc\":2}",
        )
        .expect("write profile");
        assert!(matches!(
            read_profile(path),
            Err(ProfileError::MalformedRecord { line: 1, .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn concurrent_writer_and_reader_are_rejected() {
        let dir = tempdir().expect("temp dir");
        let path = dir.path().join("runtime-v1.jsonl");
        let _writer = ProfileWriter::open(&path).expect("open first writer");

        assert!(matches!(
            ProfileWriter::open(&path),
            Err(ProfileError::AlreadyLocked { .. })
        ));
        assert!(matches!(
            read_profile(&path),
            Err(ProfileError::AlreadyLocked { .. })
        ));
    }

    #[test]
    fn personal_path_like_identifiers_are_rejected() {
        let dir = tempdir().expect("temp dir");
        let path = dir.path().join("runtime-v1.jsonl");
        let mut writer = ProfileWriter::open(path).expect("open profile");
        assert!(matches!(
            writer.record(ProfileEvent::HelperCall {
                guest_pc: 1,
                helper_id: "/Users/example/helper".to_owned(),
            }),
            Err(ProfileError::InvalidField { field: "helper_id" })
        ));
    }

    #[test]
    fn impossible_event_sizes_are_rejected() {
        let dir = tempdir().expect("temp dir");
        let path = dir.path().join("runtime-v1.jsonl");
        let mut writer = ProfileWriter::open(path).expect("open profile");
        assert!(matches!(
            writer.record(ProfileEvent::JitBlock {
                guest_pc: 1,
                code_size_bytes: 0,
                cache_file_name: "0000000000000001.nxo".to_owned(),
            }),
            Err(ProfileError::InvalidField {
                field: "code_size_bytes"
            })
        ));
        assert!(matches!(
            writer.record(ProfileEvent::Slowmem {
                guest_pc: 1,
                address: 2,
                size_bytes: 3,
                access: MemoryAccessKind::Read,
                reason_code: "page-not-fastmem".to_owned(),
            }),
            Err(ProfileError::InvalidField {
                field: "size_bytes"
            })
        ));
    }

    #[test]
    fn directory_destination_is_rejected() {
        let dir = tempdir().expect("temp dir");
        assert!(matches!(
            ProfileWriter::open(dir.path()),
            Err(ProfileError::NotRegularFile { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_destination_is_rejected() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().expect("temp dir");
        let target = dir.path().join("target.jsonl");
        let link = dir.path().join("runtime-v1.jsonl");
        fs::write(&target, b"").expect("write target");
        symlink(target, &link).expect("create symlink");
        assert!(matches!(
            ProfileWriter::open(link),
            Err(ProfileError::NotRegularFile { .. })
        ));
    }

    #[test]
    fn jit_block_candidates_filters_correctly() {
        let log = ProfileLog {
            records: all_events().into_iter().map(ProfileRecord::new).collect(),
            recovered_truncated_tail: false,
        };
        let candidates = log.jit_block_candidates();
        assert_eq!(candidates.len(), 1);
        assert_eq!(
            candidates[0],
            JitBlockCandidate {
                guest_pc: 0x1000,
                code_size_bytes: 42,
                cache_file_name: "0000000000001000.nxo",
            }
        );
    }

    #[test]
    fn jit_block_candidates_from_empty_profile() {
        let log = ProfileLog::default();
        assert!(log.jit_block_candidates().is_empty());
    }

    #[test]
    fn jit_block_candidates_deduplicates_by_guest_pc() {
        let log = ProfileLog {
            records: vec![
                ProfileRecord::new(ProfileEvent::JitBlock {
                    guest_pc: 0x1000,
                    code_size_bytes: 42,
                    cache_file_name: "0000000000001000.nxo".to_owned(),
                }),
                ProfileRecord::new(ProfileEvent::JitBlock {
                    guest_pc: 0x2000,
                    code_size_bytes: 64,
                    cache_file_name: "0000000000002000.nxo".to_owned(),
                }),
                ProfileRecord::new(ProfileEvent::JitBlock {
                    guest_pc: 0x1000,
                    code_size_bytes: 42,
                    cache_file_name: "0000000000001000.nxo".to_owned(),
                }),
            ],
            recovered_truncated_tail: false,
        };
        let candidates = log.jit_block_candidates();
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].guest_pc, 0x1000);
        assert_eq!(candidates[1].guest_pc, 0x2000);
    }
}
