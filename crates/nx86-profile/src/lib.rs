//! Versioned append-only runtime profiles (Phase 24).
//!
//! Runtime observations are persisted as newline-delimited JSON. Each line is
//! independently versioned and contains one typed event, allowing Phase 25 to
//! consume complete records even when a crash leaves the final line truncated.

use std::{
    collections::HashSet,
    fmt::Debug,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const CRATE_NAME: &str = "nx86-profile";
/// On-disk runtime profile format emitted by this crate.
pub const PROFILE_FORMAT_VERSION: u32 = 1;

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

        let existing = match fs::symlink_metadata(&path) {
            Ok(metadata) if !metadata.file_type().is_file() => {
                return Err(ProfileError::NotRegularFile { path });
            }
            Ok(_) => fs::read(&path).map_err(|source| ProfileError::io(&path, source))?,
            Err(source) if source.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(source) => return Err(ProfileError::io(&path, source)),
        };

        let parsed = parse_profile(&existing)?;
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&path)
            .map_err(|source| ProfileError::io(&path, source))?;

        match parsed.tail_repair {
            TailRepair::None => {}
            TailRepair::AddNewline => file
                .write_all(b"\n")
                .map_err(|source| ProfileError::io(&path, source))?,
            TailRepair::Truncate(valid_len) => file
                .set_len(valid_len as u64)
                .map_err(|source| ProfileError::io(&path, source))?,
        }

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
        self.file
            .write_all(&bytes)
            .map_err(|source| ProfileError::io(&self.path, source))?;
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
    let bytes = fs::read(path).map_err(|source| ProfileError::io(path, source))?;
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

        if !line.is_empty() {
            match serde_json::from_slice::<ProfileRecord>(line) {
                Ok(record) => {
                    validate_version(&record, line_number)?;
                    validate_event(&record.event)?;
                    records.push(record);
                }
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
            }
        }

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

fn validate_event(event: &ProfileEvent) -> Result<(), ProfileError> {
    match event {
        ProfileEvent::JitBlock {
            cache_file_name, ..
        } if !is_object_cache_name(cache_file_name) => Err(ProfileError::InvalidField {
            field: "cache_file_name",
        }),
        ProfileEvent::HelperCall { helper_id, .. } if !is_identifier(helper_id) => {
            Err(ProfileError::InvalidField { field: "helper_id" })
        }
        ProfileEvent::Slowmem { reason_code, .. } if !is_identifier(reason_code) => {
            Err(ProfileError::InvalidField {
                field: "reason_code",
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

/// A failure reading or writing a runtime profile.
#[derive(Debug, Error)]
pub enum ProfileError {
    #[error("profile I/O failed for {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("profile path is not a regular file: {path}")]
    NotRegularFile { path: PathBuf },
    #[error("profile record serialization failed: {0}")]
    Serialize(serde_json::Error),
    #[error("profile record on line {line} is malformed: {source}")]
    MalformedRecord {
        line: usize,
        source: serde_json::Error,
    },
    #[error("profile record on line {line} uses unsupported format version {found}")]
    UnsupportedVersion { line: usize, found: u32 },
    #[error("profile field `{field}` is not a safe deterministic identifier")]
    InvalidField { field: &'static str },
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
        MemoryAccessKind, ProfileError, ProfileEvent, ProfileRecord, ProfileSink, ProfileWriter,
        RecordOutcome, read_profile,
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
}
