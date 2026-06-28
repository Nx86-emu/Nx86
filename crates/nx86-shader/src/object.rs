//! Self-describing, integrity-checked `.nxshader` object format.
//!
//! Mirrors the `.nxo` native-object layout in `nx86-object`: a compact
//! little-endian header followed by the translated body and a trailing FNV-1a
//! content hash.
//!
//! ```text
//! off  size  field
//! 0    4     magic = b"NXS\0"
//! 4    4     version       (u32)
//! 8    4     stage_code    (u32)   ShaderStage::code
//! 12   8     source_hash   (u64)   FNV-1a of the original source (cache key)
//! 20   4     translated_len(u32)
//! 24   ..    translated bytes
//! end  8     content_hash  (u64)   FNV-1a 64 over every preceding byte
//! ```

use std::{fs, io, path::Path};

use thiserror::Error;

use crate::{ShaderError, ShaderHash, ShaderStage};

/// Magic bytes at the start of every `.nxshader` object.
pub const SHADER_OBJECT_MAGIC: [u8; 4] = *b"NXS\0";
/// Current `.nxshader` object format version.
pub const SHADER_OBJECT_VERSION: u32 = 1;

const HEADER_LEN: usize = 24;
/// Length of the fixed `.nxshader` header preceding the body and trailing hash.
pub const SHADER_OBJECT_HEADER_LEN: usize = HEADER_LEN;
const HASH_LEN: usize = 8;

const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// A translated shader plus the stage and source hash needed to reload and
/// identify it. The `source_hash` is the cache key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShaderObject {
    pub stage: ShaderStage,
    pub source_hash: ShaderHash,
    pub translated: Vec<u8>,
}

impl ShaderObject {
    /// Serialize to the `.nxshader` byte layout, appending the content hash.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let translated_len = u32::try_from(self.translated.len()).unwrap_or(u32::MAX);
        let mut out = Vec::with_capacity(HEADER_LEN + self.translated.len() + HASH_LEN);
        out.extend_from_slice(&SHADER_OBJECT_MAGIC);
        out.extend_from_slice(&SHADER_OBJECT_VERSION.to_le_bytes());
        out.extend_from_slice(&self.stage.code().to_le_bytes());
        out.extend_from_slice(&self.source_hash.as_u64().to_le_bytes());
        out.extend_from_slice(&translated_len.to_le_bytes());
        out.extend_from_slice(&self.translated);
        let hash = fnv1a(&out);
        out.extend_from_slice(&hash.to_le_bytes());
        out
    }

    /// Parse just the fixed header, validating the magic but not the version or
    /// content hash — the cheap inspection the cache scan uses.
    pub fn read_header(bytes: &[u8]) -> Result<ShaderObjectHeader, ShaderObjectError> {
        let magic: [u8; 4] = bytes
            .get(0..4)
            .and_then(|slice| slice.try_into().ok())
            .ok_or(ShaderObjectError::Truncated)?;
        if magic != SHADER_OBJECT_MAGIC {
            return Err(ShaderObjectError::BadMagic);
        }
        Ok(ShaderObjectHeader {
            version: read_u32(bytes, 4)?,
            stage_code: read_u32(bytes, 8)?,
            source_hash: ShaderHash(read_u64(bytes, 12)?),
            translated_len: read_u32(bytes, 20)?,
        })
    }

    /// Parse and validate a `.nxshader` buffer (magic, version, stage code, exact
    /// length, and content hash).
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ShaderObjectError> {
        if bytes.len() < HEADER_LEN + HASH_LEN {
            return Err(ShaderObjectError::Truncated);
        }
        let header = Self::read_header(bytes)?;
        if header.version != SHADER_OBJECT_VERSION {
            return Err(ShaderObjectError::UnsupportedVersion {
                found: header.version,
            });
        }
        let translated_len = header.translated_len as usize;

        let expected_len = HEADER_LEN
            .checked_add(translated_len)
            .and_then(|value| value.checked_add(HASH_LEN))
            .ok_or(ShaderObjectError::Truncated)?;
        if bytes.len() != expected_len {
            return Err(ShaderObjectError::Truncated);
        }

        let translated = bytes
            .get(HEADER_LEN..HEADER_LEN + translated_len)
            .ok_or(ShaderObjectError::Truncated)?
            .to_vec();
        let stored_hash = read_u64(bytes, HEADER_LEN + translated_len)?;
        let computed = fnv1a(&bytes[..bytes.len() - HASH_LEN]);
        if computed != stored_hash {
            return Err(ShaderObjectError::HashMismatch {
                expected: stored_hash,
                actual: computed,
            });
        }

        // Decode the stage only after length and content-hash validation, so a
        // corrupt or truncated object reports `Truncated`/`HashMismatch` in
        // preference to an unknown-stage error.
        let stage = ShaderStage::from_code(header.stage_code)?;
        Ok(Self {
            stage,
            source_hash: header.source_hash,
            translated,
        })
    }

    /// Write the serialized object to `path`.
    pub fn write_to_path(&self, path: &Path) -> Result<(), ShaderObjectError> {
        fs::write(path, self.to_bytes()).map_err(ShaderObjectError::Io)
    }

    /// Read and validate an object from `path`.
    pub fn read_from_path(path: &Path) -> Result<Self, ShaderObjectError> {
        let bytes = fs::read(path).map_err(ShaderObjectError::Io)?;
        Self::from_bytes(&bytes)
    }

    /// Conventional file name for this object, keyed by its source hash.
    #[must_use]
    pub fn file_name(&self) -> String {
        shader_object_file_name(self.source_hash)
    }
}

/// The fixed-size `.nxshader` header, parsed without loading or hash-validating
/// the body. See [`ShaderObject::read_header`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShaderObjectHeader {
    pub version: u32,
    pub stage_code: u32,
    pub source_hash: ShaderHash,
    pub translated_len: u32,
}

/// Conventional `.nxshader` file name for an object with the given source hash.
/// The shared source of truth for both writing and cache lookup.
#[must_use]
pub fn shader_object_file_name(source_hash: ShaderHash) -> String {
    format!("{}.nxshader", source_hash.hex())
}

/// A failure parsing or loading a `.nxshader` object.
#[derive(Debug, Error)]
pub enum ShaderObjectError {
    #[error("object magic does not match the .nxshader format")]
    BadMagic,
    #[error("unsupported .nxshader object version {found}")]
    UnsupportedVersion { found: u32 },
    #[error("shader object data is truncated or malformed")]
    Truncated,
    #[error(transparent)]
    Stage(#[from] ShaderError),
    #[error(
        "shader object validation hash mismatch: stored {expected:#018x}, computed {actual:#018x}"
    )]
    HashMismatch { expected: u64, actual: u64 },
    #[error("shader object file I/O failed: {0}")]
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

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, ShaderObjectError> {
    let end = offset.checked_add(4).ok_or(ShaderObjectError::Truncated)?;
    let array: [u8; 4] = bytes
        .get(offset..end)
        .and_then(|slice| slice.try_into().ok())
        .ok_or(ShaderObjectError::Truncated)?;
    Ok(u32::from_le_bytes(array))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, ShaderObjectError> {
    let end = offset.checked_add(8).ok_or(ShaderObjectError::Truncated)?;
    let array: [u8; 8] = bytes
        .get(offset..end)
        .and_then(|slice| slice.try_into().ok())
        .ok_or(ShaderObjectError::Truncated)?;
    Ok(u64::from_le_bytes(array))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::translate;

    fn sample() -> ShaderObject {
        translate(ShaderStage::Fragment, b"void main() {}", "main").to_object()
    }

    #[test]
    fn round_trips_through_bytes() {
        let object = sample();
        let restored = ShaderObject::from_bytes(&object.to_bytes()).expect("valid object");
        assert_eq!(restored, object);
    }

    #[test]
    fn file_name_uses_source_hash() {
        let object = sample();
        assert_eq!(
            object.file_name(),
            format!("{}.nxshader", object.source_hash.hex())
        );
    }

    #[test]
    fn magic_is_nxs() {
        assert_eq!(&SHADER_OBJECT_MAGIC, b"NXS\0");
    }

    #[test]
    fn rejects_bad_magic() {
        let mut bytes = sample().to_bytes();
        bytes[0] = b'X';
        assert!(matches!(
            ShaderObject::from_bytes(&bytes),
            Err(ShaderObjectError::BadMagic)
        ));
    }

    #[test]
    fn rejects_unsupported_version() {
        let mut bytes = sample().to_bytes();
        bytes[4] = 0xFF;
        assert!(matches!(
            ShaderObject::from_bytes(&bytes),
            Err(ShaderObjectError::UnsupportedVersion { .. })
        ));
    }

    #[test]
    fn rejects_truncated() {
        let bytes = sample().to_bytes();
        let truncated = &bytes[..bytes.len() - 4];
        assert!(matches!(
            ShaderObject::from_bytes(truncated),
            Err(ShaderObjectError::Truncated)
        ));
    }

    #[test]
    fn detects_corruption() {
        let mut bytes = sample().to_bytes();
        // Flip the first translated byte (offset 24).
        bytes[24] ^= 0xFF;
        assert!(matches!(
            ShaderObject::from_bytes(&bytes),
            Err(ShaderObjectError::HashMismatch { .. })
        ));
    }

    #[test]
    fn read_header_parses_without_hash() {
        let object = sample();
        let mut bytes = object.to_bytes();
        bytes[24] ^= 0xFF;
        let header = ShaderObject::read_header(&bytes).expect("header should parse");
        assert_eq!(header.version, SHADER_OBJECT_VERSION);
        assert_eq!(header.stage_code, object.stage.code());
        assert_eq!(header.source_hash, object.source_hash);
        assert_eq!(header.translated_len as usize, object.translated.len());
    }
}
