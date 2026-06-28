//! Shader translation skeleton (Phase 49).
//!
//! This crate is the GPU-side parallel of the CPU AOT object/cache stack
//! (`nx86-object` + `nx86-cache`): it models a shader, hashes its source,
//! "translates" it to a **deterministic placeholder** (no real SPIR-V/Maxwell
//! yet — that arrives in Phase 50), serializes the result to a self-describing,
//! integrity-checked `.nxshader` object, and caches it under a title's
//! `cache/shaders/` folder.
//!
//! It is pure logic plus `std` file I/O, so it is host-independent and fully
//! testable on the dev host. Phase 50 adds the [`compile_shaders`] batch pass
//! that compiles a whole shader set during initial compile and reports
//! **shader readiness**;
//! that readiness is folded into Native Coverage by `nx86-core`'s coverage
//! combiner.

mod aot;
mod cache;
mod object;

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

pub use aot::{
    ShaderAotInput, ShaderAotReport, ShaderHint, ShaderProfileHints, cached_readiness_bps,
    compile_shaders,
};
pub use cache::{
    SHADER_MANIFEST_FILE, ShaderCache, ShaderCacheEntry, ShaderCacheError, ShaderCacheManifest,
    ShaderCacheStatus, ShaderCheckOutcome,
};
pub use object::{
    SHADER_OBJECT_HEADER_LEN, SHADER_OBJECT_MAGIC, SHADER_OBJECT_VERSION, ShaderObject,
    ShaderObjectError, ShaderObjectHeader, shader_object_file_name,
};

pub const CRATE_NAME: &str = "nx86-shader";

#[must_use]
pub const fn crate_name() -> &'static str {
    CRATE_NAME
}

/// Version of the [`ShaderMetadata`] model, bumped on incompatible changes.
/// Mirrors the `PROFILE_FORMAT_VERSION` convention in `nx86-profile`.
pub const SHADER_FORMAT_VERSION: u32 = 1;

const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// The programmable pipeline stage a shader targets.
///
/// Stored as a string in the synthetic shader spec and as a small integer code
/// in the binary `.nxshader` header (see [`ShaderStage::code`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ShaderStage {
    Vertex,
    Fragment,
    Compute,
}

impl ShaderStage {
    /// Stable integer code used in the binary object header.
    #[must_use]
    pub const fn code(self) -> u32 {
        match self {
            Self::Vertex => 0,
            Self::Fragment => 1,
            Self::Compute => 2,
        }
    }

    /// Recover a stage from its [`ShaderStage::code`].
    pub fn from_code(code: u32) -> Result<Self, ShaderError> {
        match code {
            0 => Ok(Self::Vertex),
            1 => Ok(Self::Fragment),
            2 => Ok(Self::Compute),
            other => Err(ShaderError::UnknownStageCode(other)),
        }
    }

    /// Lowercase canonical name, matching the kebab-case serde representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Vertex => "vertex",
            Self::Fragment => "fragment",
            Self::Compute => "compute",
        }
    }
}

impl fmt::Display for ShaderStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ShaderStage {
    type Err = ShaderError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "vertex" | "vert" | "vs" => Ok(Self::Vertex),
            "fragment" | "frag" | "fs" | "pixel" => Ok(Self::Fragment),
            "compute" | "comp" | "cs" => Ok(Self::Compute),
            _ => Err(ShaderError::UnknownStage(value.to_owned())),
        }
    }
}

/// FNV-1a-64 identity/integrity hash of a shader's source bytes. Dependency-free
/// and deterministic, matching the hashing used by `.nxo` objects. Not
/// cryptographic.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ShaderHash(pub u64);

impl ShaderHash {
    /// Hash `source` with FNV-1a-64.
    #[must_use]
    pub fn of(source: &[u8]) -> Self {
        let mut hash = FNV_OFFSET_BASIS;
        for &byte in source {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(FNV_PRIME);
        }
        Self(hash)
    }

    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    /// Zero-padded 16-digit hex, used for the on-disk file name.
    #[must_use]
    pub fn hex(self) -> String {
        format!("{:016x}", self.0)
    }
}

impl fmt::Display for ShaderHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

/// How far a shader has been compiled. Phase 49 only produces
/// [`TranslationStatus::Placeholder`]; later phases add real SPIR-V/native tiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TranslationStatus {
    /// A deterministic stand-in produced without real shader compilation.
    Placeholder,
}

/// The descriptive model of a translated shader (the "shader metadata model").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ShaderMetadata {
    pub format_version: u32,
    pub stage: ShaderStage,
    pub source_hash: ShaderHash,
    pub entry: String,
    pub source_len: u64,
    pub translated_len: u64,
    pub status: TranslationStatus,
}

impl ShaderMetadata {
    /// Validate the format version on load, mirroring `nx86-profile`'s
    /// `validate_version`.
    pub fn validate_version(&self) -> Result<(), ShaderError> {
        if self.format_version == SHADER_FORMAT_VERSION {
            Ok(())
        } else {
            Err(ShaderError::UnsupportedFormatVersion {
                found: self.format_version,
            })
        }
    }
}

/// The output of the placeholder translation path: descriptive metadata plus the
/// deterministic translated bytes ready to be cached as a [`ShaderObject`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranslatedShader {
    pub metadata: ShaderMetadata,
    pub translated: Vec<u8>,
}

impl TranslatedShader {
    /// Package this translation into a cacheable `.nxshader` object.
    #[must_use]
    pub fn to_object(&self) -> ShaderObject {
        ShaderObject {
            stage: self.metadata.stage,
            source_hash: self.metadata.source_hash,
            translated: self.translated.clone(),
        }
    }
}

/// Placeholder tag prefixing every translated blob, so the bytes are
/// recognizable and deterministic without being valid SPIR-V.
const PLACEHOLDER_TAG: &[u8; 12] = b"NXSHADER-PH\0";

/// Translate a shader source to a deterministic placeholder.
///
/// Phase 49's "translation" produces a stable byte blob — the placeholder tag,
/// the stage code, and the source hash — not real SPIR-V. The seam exists so
/// Phase 50 can replace the body with an actual compiler while everything around
/// it (hashing, the `.nxshader` container, caching) stays put.
#[must_use]
pub fn translate(stage: ShaderStage, source: &[u8], entry: &str) -> TranslatedShader {
    let source_hash = ShaderHash::of(source);
    let mut translated = Vec::with_capacity(PLACEHOLDER_TAG.len() + 12);
    translated.extend_from_slice(PLACEHOLDER_TAG);
    translated.extend_from_slice(&stage.code().to_le_bytes());
    translated.extend_from_slice(&source_hash.as_u64().to_le_bytes());
    let metadata = ShaderMetadata {
        format_version: SHADER_FORMAT_VERSION,
        stage,
        source_hash,
        entry: entry.to_owned(),
        source_len: source.len() as u64,
        translated_len: translated.len() as u64,
        status: TranslationStatus::Placeholder,
    };
    TranslatedShader {
        metadata,
        translated,
    }
}

/// Errors from shader modeling, stage parsing, and translation.
#[derive(Debug, thiserror::Error)]
pub enum ShaderError {
    #[error("unknown shader stage '{0}'")]
    UnknownStage(String),
    #[error("unknown shader stage code {0}")]
    UnknownStageCode(u32),
    #[error("unsupported shader metadata version {found}")]
    UnsupportedFormatVersion { found: u32 },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_parses_and_round_trips_codes() {
        assert_eq!(
            "frag".parse::<ShaderStage>().expect("frag"),
            ShaderStage::Fragment
        );
        assert_eq!(
            "VERTEX".parse::<ShaderStage>().expect("vertex"),
            ShaderStage::Vertex
        );
        for stage in [
            ShaderStage::Vertex,
            ShaderStage::Fragment,
            ShaderStage::Compute,
        ] {
            assert_eq!(ShaderStage::from_code(stage.code()).expect("code"), stage);
        }
        assert!("geometry".parse::<ShaderStage>().is_err());
        assert!(ShaderStage::from_code(99).is_err());
    }

    #[test]
    fn shader_hash_is_deterministic_and_padded() {
        let a = ShaderHash::of(b"void main() {}");
        let b = ShaderHash::of(b"void main() {}");
        let c = ShaderHash::of(b"void other() {}");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.hex().len(), 16);
    }

    #[test]
    fn translate_is_deterministic_and_marks_placeholder() {
        let one = translate(ShaderStage::Vertex, b"source", "main");
        let two = translate(ShaderStage::Vertex, b"source", "main");
        assert_eq!(one, two);
        assert_eq!(one.metadata.status, TranslationStatus::Placeholder);
        assert_eq!(one.metadata.source_hash, ShaderHash::of(b"source"));
        assert_eq!(one.metadata.source_len, 6);
        assert_eq!(one.metadata.translated_len as usize, one.translated.len());
        // Distinct stages or sources yield distinct translated bytes.
        assert_ne!(one, translate(ShaderStage::Fragment, b"source", "main"));
        assert_ne!(
            one.translated,
            translate(ShaderStage::Vertex, b"other", "main").translated
        );
    }

    #[test]
    fn metadata_version_validation() {
        let mut meta = translate(ShaderStage::Compute, b"x", "main").metadata;
        assert!(meta.validate_version().is_ok());
        meta.format_version = 999;
        assert!(meta.validate_version().is_err());
    }
}
