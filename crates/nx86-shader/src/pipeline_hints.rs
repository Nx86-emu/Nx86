//! Pipeline profile hints (Phase 51).
//!
//! A [`PipelineProfileHints`] is a versioned, serde-serializable set of
//! [`PipelineHint`]s that MAY assist pipeline cache warming. Each hint
//! identifies a pipeline by its [`PipelineKey`] (a deterministic hash of the
//! shader combination + render state) and whether it was observed hot. This
//! mirrors the [`ShaderProfileHints`] pattern used for shader AOT.

use serde::{Deserialize, Serialize};

use crate::{FNV_OFFSET_BASIS, FNV_PRIME, ShaderHash};

/// Format version for the pipeline profile hints file.
pub const PIPELINE_HINTS_FORMAT_VERSION: u32 = 1;

/// Stable identifier for a Vulkan graphics or compute pipeline, derived from
/// the shader combination and render state descriptor. Not cryptographic.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PipelineKey(pub u64);

impl PipelineKey {
    /// Derive a pipeline key from a set of shader hashes and an opaque state
    /// descriptor. The result is deterministic for the same inputs.
    #[must_use]
    pub fn from_shaders(shader_hashes: &[ShaderHash], state_descriptor: &[u8]) -> Self {
        let mut hash = FNV_OFFSET_BASIS;
        for h in shader_hashes {
            for byte in h.as_u64().to_le_bytes() {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(FNV_PRIME);
            }
        }
        for &byte in state_descriptor {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(FNV_PRIME);
        }
        Self(hash)
    }

    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

/// One shared-profile hint about a pipeline the title is known to use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct PipelineHint {
    /// Pipeline key identifying the pipeline (its cache key).
    pub pipeline_key: PipelineKey,
    /// Whether the pipeline was observed hot (compiled first by the AOT pass).
    pub hot: bool,
}

/// Shared-profile hints that MAY assist pipeline cache warming (Phase 51).
/// Mirrors `ShaderProfileHints` in structure and conventions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct PipelineProfileHints {
    pub format_version: u32,
    pub entries: Vec<PipelineHint>,
}

impl Default for PipelineProfileHints {
    fn default() -> Self {
        Self {
            format_version: PIPELINE_HINTS_FORMAT_VERSION,
            entries: Vec::new(),
        }
    }
}

impl PipelineProfileHints {
    /// Empty hints at the current format version.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Build hints from a list of entries at the current format version.
    #[must_use]
    pub fn from_entries(entries: Vec<PipelineHint>) -> Self {
        Self {
            format_version: PIPELINE_HINTS_FORMAT_VERSION,
            entries,
        }
    }

    /// Validate the format version on load.
    pub fn validate_version(&self) -> Result<(), PipelineHintError> {
        if self.format_version == PIPELINE_HINTS_FORMAT_VERSION {
            Ok(())
        } else {
            Err(PipelineHintError::UnsupportedFormatVersion {
                found: self.format_version,
            })
        }
    }

    /// Whether `pipeline_key` is hinted hot.
    #[must_use]
    pub fn is_hot(&self, pipeline_key: PipelineKey) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.pipeline_key == pipeline_key && entry.hot)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PipelineHintError {
    #[error("unsupported pipeline hints format version {found}")]
    UnsupportedFormatVersion { found: u32 },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipeline_key_is_deterministic() {
        let a = PipelineKey::from_shaders(
            &[ShaderHash::of(b"vert"), ShaderHash::of(b"frag")],
            b"state",
        );
        let b = PipelineKey::from_shaders(
            &[ShaderHash::of(b"vert"), ShaderHash::of(b"frag")],
            b"state",
        );
        assert_eq!(a, b);

        // Different state yields different key.
        let c = PipelineKey::from_shaders(
            &[ShaderHash::of(b"vert"), ShaderHash::of(b"frag")],
            b"other",
        );
        assert_ne!(a, c);
    }

    #[test]
    fn hints_version_validates() {
        let hints = PipelineProfileHints::new();
        assert!(hints.validate_version().is_ok());

        let mut bad = PipelineProfileHints::new();
        bad.format_version = 999;
        assert!(bad.validate_version().is_err());
    }

    #[test]
    fn hot_lookup_works() {
        let key = PipelineKey::from_shaders(&[ShaderHash::of(b"s")], b"");
        let hints = PipelineProfileHints::from_entries(vec![PipelineHint {
            pipeline_key: key,
            hot: true,
        }]);
        assert!(hints.is_hot(key));

        let other = PipelineKey(0);
        assert!(!hints.is_hot(other));
    }

    #[test]
    fn serde_round_trips() {
        let hints = PipelineProfileHints::from_entries(vec![PipelineHint {
            pipeline_key: PipelineKey(42),
            hot: false,
        }]);
        let json = serde_json::to_string(&hints).expect("serialize");
        let back: PipelineProfileHints = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(hints, back);
    }
}
