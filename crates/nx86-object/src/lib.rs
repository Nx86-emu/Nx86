//! AOT object format v0 (Phase 20).
//!
//! A [`NativeObject`] is a single lowered native block serialized to a
//! self-describing, integrity-checked `.nxo` file so a compiled block can
//! persist across restarts. The format is a compact little-endian layout with a
//! trailing FNV-1a content hash:
//!
//! ```text
//! off  size  field
//! 0    4     magic = b"NXO\0"
//! 4    4     version (u32)
//! 8    8     entry_address (u64)   guest mapping: first guest PC
//! 16   8     guest_end     (u64)   guest mapping: exclusive end PC
//! 24   4     stack_size    (u32)   frame metadata
//! 28   4     code_len      (u32)
//! 32   ..    code bytes
//! end  8     content_hash  (u64)   FNV-1a 64 over every preceding byte
//! ```
//!
//! The hash is a dependency-free integrity/identity check, not a cryptographic
//! one; the Phase 21 cache "full check" can upgrade it. This crate is pure logic
//! plus std file I/O, so it is host-independent.

use std::{fs, io, path::Path};

use thiserror::Error;

pub const CRATE_NAME: &str = "nx86-object";

#[must_use]
pub const fn crate_name() -> &'static str {
    CRATE_NAME
}

/// Magic bytes at the start of every `.nxo` object.
pub const OBJECT_MAGIC: [u8; 4] = *b"NXO\0";
/// Current object format version.
pub const OBJECT_VERSION: u32 = 1;

const HEADER_LEN: usize = 32;
/// Length of the fixed `.nxo` header (magic, version, guest mapping, stack size,
/// and code length) that precedes the code bytes and trailing hash.
pub const OBJECT_HEADER_LEN: usize = HEADER_LEN;
const HASH_LEN: usize = 8;

const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// A single lowered native block plus the guest mapping needed to reload and
/// (later) dispatch it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeObject {
    /// First guest PC this block was lifted from.
    pub entry_address: u64,
    /// Exclusive guest PC at which the block's coverage ends.
    pub guest_end: u64,
    /// Stack frame size the lowerer reserved (metadata; the prologue/epilogue
    /// are already inline in `code`).
    pub stack_size: u32,
    /// Generated x86_64 machine code.
    pub code: Vec<u8>,
}

impl NativeObject {
    /// Serialize to the `.nxo` byte layout, appending the content hash.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let code_len = u32::try_from(self.code.len()).unwrap_or(u32::MAX);
        let mut out = Vec::with_capacity(HEADER_LEN + self.code.len() + HASH_LEN);
        out.extend_from_slice(&OBJECT_MAGIC);
        out.extend_from_slice(&OBJECT_VERSION.to_le_bytes());
        out.extend_from_slice(&self.entry_address.to_le_bytes());
        out.extend_from_slice(&self.guest_end.to_le_bytes());
        out.extend_from_slice(&self.stack_size.to_le_bytes());
        out.extend_from_slice(&code_len.to_le_bytes());
        out.extend_from_slice(&self.code);
        let hash = fnv1a(&out);
        out.extend_from_slice(&hash.to_le_bytes());
        out
    }

    /// Parse just the fixed header from the start of a `.nxo` buffer, validating
    /// the magic but not the version or content hash. This is the cheap
    /// "shallow" inspection the cache uses to enumerate objects without loading
    /// and hashing their code.
    pub fn read_header(bytes: &[u8]) -> Result<ObjectHeader, ObjectError> {
        let magic: [u8; 4] = bytes
            .get(0..4)
            .and_then(|slice| slice.try_into().ok())
            .ok_or(ObjectError::Truncated)?;
        if magic != OBJECT_MAGIC {
            return Err(ObjectError::BadMagic);
        }
        Ok(ObjectHeader {
            version: read_u32(bytes, 4)?,
            entry_address: read_u64(bytes, 8)?,
            guest_end: read_u64(bytes, 16)?,
            stack_size: read_u32(bytes, 24)?,
            code_len: read_u32(bytes, 28)?,
        })
    }

    /// Parse and validate a `.nxo` byte buffer (magic, version, exact length,
    /// and content hash).
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ObjectError> {
        if bytes.len() < HEADER_LEN + HASH_LEN {
            return Err(ObjectError::Truncated);
        }
        let header = Self::read_header(bytes)?;
        if header.version != OBJECT_VERSION {
            return Err(ObjectError::UnsupportedVersion {
                found: header.version,
            });
        }
        let code_len = header.code_len as usize;

        let expected_len = HEADER_LEN
            .checked_add(code_len)
            .and_then(|value| value.checked_add(HASH_LEN))
            .ok_or(ObjectError::Truncated)?;
        if bytes.len() != expected_len {
            return Err(ObjectError::Truncated);
        }

        let code = bytes
            .get(HEADER_LEN..HEADER_LEN + code_len)
            .ok_or(ObjectError::Truncated)?
            .to_vec();
        let stored_hash = read_u64(bytes, HEADER_LEN + code_len)?;
        let computed = fnv1a(&bytes[..bytes.len() - HASH_LEN]);
        if computed != stored_hash {
            return Err(ObjectError::HashMismatch {
                expected: stored_hash,
                actual: computed,
            });
        }

        Ok(Self {
            entry_address: header.entry_address,
            guest_end: header.guest_end,
            stack_size: header.stack_size,
            code,
        })
    }

    /// Write the serialized object to `path`.
    pub fn write_to_path(&self, path: &Path) -> Result<(), ObjectError> {
        fs::write(path, self.to_bytes()).map_err(ObjectError::Io)
    }

    /// Read and validate an object from `path`.
    pub fn read_from_path(path: &Path) -> Result<Self, ObjectError> {
        let bytes = fs::read(path).map_err(ObjectError::Io)?;
        Self::from_bytes(&bytes)
    }

    /// Conventional file name for this object, keyed by its guest entry address.
    #[must_use]
    pub fn file_name(&self) -> String {
        object_file_name(self.entry_address)
    }
}

/// The fixed-size header at the start of every `.nxo` object, parsed without
/// loading or hash-validating the code body. See [`NativeObject::read_header`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjectHeader {
    pub version: u32,
    pub entry_address: u64,
    pub guest_end: u64,
    pub stack_size: u32,
    pub code_len: u32,
}

/// Conventional `.nxo` file name for an object with the given guest entry
/// address. The shared source of truth for both writing and cache lookup.
#[must_use]
pub fn object_file_name(entry_address: u64) -> String {
    format!("{entry_address:016x}.nxo")
}

/// A failure parsing or loading a `.nxo` object.
#[derive(Debug, Error)]
pub enum ObjectError {
    #[error("object magic does not match the .nxo format")]
    BadMagic,
    #[error("unsupported .nxo object version {found}")]
    UnsupportedVersion { found: u32 },
    #[error("object data is truncated or malformed")]
    Truncated,
    #[error("object validation hash mismatch: stored {expected:#018x}, computed {actual:#018x}")]
    HashMismatch { expected: u64, actual: u64 },
    #[error("object file I/O failed: {0}")]
    Io(io::Error),
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET_BASIS;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, ObjectError> {
    let end = offset.checked_add(4).ok_or(ObjectError::Truncated)?;
    let array: [u8; 4] = bytes
        .get(offset..end)
        .and_then(|slice| slice.try_into().ok())
        .ok_or(ObjectError::Truncated)?;
    Ok(u32::from_le_bytes(array))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, ObjectError> {
    let end = offset.checked_add(8).ok_or(ObjectError::Truncated)?;
    let array: [u8; 8] = bytes
        .get(offset..end)
        .and_then(|slice| slice.try_into().ok())
        .ok_or(ObjectError::Truncated)?;
    Ok(u64::from_le_bytes(array))
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::{NativeObject, OBJECT_MAGIC, ObjectError};

    fn sample() -> NativeObject {
        NativeObject {
            entry_address: 0x4000,
            guest_end: 0x4010,
            stack_size: 32,
            code: vec![0x48, 0xB8, 0x2A, 0, 0, 0, 0, 0, 0, 0, 0xC3],
        }
    }

    #[test]
    fn round_trips_through_bytes() {
        let object = sample();
        let restored = NativeObject::from_bytes(&object.to_bytes()).expect("valid object");
        assert_eq!(restored, object);
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = tempdir().expect("temp dir");
        let object = sample();
        let path = dir.path().join(object.file_name());

        object.write_to_path(&path).expect("write object");
        let restored = NativeObject::read_from_path(&path).expect("read object");

        assert_eq!(restored, object);
    }

    #[test]
    fn file_name_uses_entry_address() {
        assert_eq!(sample().file_name(), "0000000000004000.nxo");
    }

    #[test]
    fn magic_is_nxo() {
        assert_eq!(&OBJECT_MAGIC, b"NXO\0");
    }

    #[test]
    fn rejects_bad_magic() {
        let mut bytes = sample().to_bytes();
        bytes[0] = b'X';
        assert!(matches!(
            NativeObject::from_bytes(&bytes),
            Err(ObjectError::BadMagic)
        ));
    }

    #[test]
    fn rejects_unsupported_version() {
        let mut bytes = sample().to_bytes();
        // Version occupies offset 4..8; bump it before the hash is checked.
        bytes[4] = 0xFF;
        assert!(matches!(
            NativeObject::from_bytes(&bytes),
            Err(ObjectError::UnsupportedVersion { .. })
        ));
    }

    #[test]
    fn rejects_truncated() {
        let bytes = sample().to_bytes();
        let truncated = &bytes[..bytes.len() - 4];
        assert!(matches!(
            NativeObject::from_bytes(truncated),
            Err(ObjectError::Truncated)
        ));
    }

    #[test]
    fn detects_corruption() {
        let mut bytes = sample().to_bytes();
        // Flip the first code byte (offset 32).
        bytes[32] ^= 0xFF;
        assert!(matches!(
            NativeObject::from_bytes(&bytes),
            Err(ObjectError::HashMismatch { .. })
        ));
    }

    #[test]
    fn read_header_parses_mapping_without_hash() {
        let object = sample();
        // A corrupt body still yields a readable header (no hash validation).
        let mut bytes = object.to_bytes();
        bytes[32] ^= 0xFF;

        let header = NativeObject::read_header(&bytes).expect("header should parse");
        assert_eq!(header.version, super::OBJECT_VERSION);
        assert_eq!(header.entry_address, object.entry_address);
        assert_eq!(header.guest_end, object.guest_end);
        assert_eq!(header.stack_size, object.stack_size);
        assert_eq!(header.code_len as usize, object.code.len());
    }

    #[test]
    fn read_header_rejects_bad_magic() {
        let mut bytes = sample().to_bytes();
        bytes[0] = b'X';
        assert!(matches!(
            NativeObject::read_header(&bytes),
            Err(ObjectError::BadMagic)
        ));
    }
}
