//! Batch shader AOT pass (Phase 50).
//!
//! Turns the Phase 49 single-shader primitives (`translate` + `ShaderCache`)
//! into the "compile shaders during initial compile" step: it translates a
//! title's whole shader set to placeholders, caches each as a `.nxshader`
//! object, and reports **shader readiness** (how many of the set are cached and
//! valid). Optional [`ShaderProfileHints`] let a shared profile steer the pass
//! (hot shaders first) — the GPU-side parallel of how the CPU `ProfileLog`
//! drives `nx86-backend::rebuild_from_profile`.
//!
//! Pure logic plus `std` file I/O, so it is host-independent and fully testable
//! on the dev host. The translation remains the deterministic placeholder — no
//! real SPIR-V/Maxwell yet.

use serde::{Deserialize, Serialize};

use crate::cache::{ShaderCache, ShaderCacheError, ShaderCheckOutcome};
use crate::{SHADER_FORMAT_VERSION, ShaderError, ShaderHash, ShaderStage, translate};

/// One shared-profile hint about a shader the title is known to use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ShaderHint {
    /// Source hash identifying the shader (its cache key).
    pub source_hash: ShaderHash,
    /// Pipeline stage the shader targets.
    pub stage: ShaderStage,
    /// Whether the shader was observed hot (compiled first by the AOT pass).
    pub hot: bool,
}

/// Shared-profile hints that MAY assist shader AOT (SPEC §33.2). A
/// `format_version`'d, serde-serializable set of [`ShaderHint`]s, mirroring the
/// `PROFILE_FORMAT_VERSION` convention used elsewhere.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ShaderProfileHints {
    pub format_version: u32,
    pub entries: Vec<ShaderHint>,
}

impl Default for ShaderProfileHints {
    fn default() -> Self {
        Self {
            format_version: SHADER_FORMAT_VERSION,
            entries: Vec::new(),
        }
    }
}

impl ShaderProfileHints {
    /// Empty hints at the current format version.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Build hints from a list of entries at the current format version.
    #[must_use]
    pub fn from_entries(entries: Vec<ShaderHint>) -> Self {
        Self {
            format_version: SHADER_FORMAT_VERSION,
            entries,
        }
    }

    /// Validate the format version on load, mirroring `ShaderMetadata`.
    pub fn validate_version(&self) -> Result<(), ShaderError> {
        if self.format_version == SHADER_FORMAT_VERSION {
            Ok(())
        } else {
            Err(ShaderError::UnsupportedFormatVersion {
                found: self.format_version,
            })
        }
    }

    /// Whether `source_hash` is hinted hot (any matching, hot entry).
    #[must_use]
    pub fn is_hot(&self, source_hash: ShaderHash) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.source_hash == source_hash && entry.hot)
    }
}

/// A shader the AOT pass should compile: its stage, opaque source bytes, and
/// entry-point name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShaderAotInput {
    pub stage: ShaderStage,
    pub source: Vec<u8>,
    pub entry: String,
}

impl ShaderAotInput {
    #[must_use]
    pub fn new(stage: ShaderStage, source: Vec<u8>, entry: impl Into<String>) -> Self {
        Self {
            stage,
            source,
            entry: entry.into(),
        }
    }

    /// The cache key this input translates to.
    #[must_use]
    pub fn source_hash(&self) -> ShaderHash {
        ShaderHash::of(&self.source)
    }
}

/// The outcome of a batch shader AOT pass — what was compiled and how ready the
/// shader set now is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShaderAotReport {
    /// Shaders the pass attempted (the input set size).
    pub total: usize,
    /// Shaders freshly translated and inserted this pass.
    pub compiled: usize,
    /// Shaders already present and valid in the cache (skipped re-translation).
    pub already_cached: usize,
    /// Shaders now present and valid in the cache (`compiled + already_cached`).
    pub cached_ok: usize,
    /// Shaders whose cache insert failed.
    pub failed: usize,
    /// Hot-hinted shaders in the input set.
    pub hot_total: usize,
    /// Hot-hinted shaders now cached and valid.
    pub hot_cached_ok: usize,
    /// Diagnostic messages from failed inserts (one per failure).
    pub errors: Vec<String>,
    /// Processing order (hot-first), by source hash — for inspection/tests.
    pub order: Vec<ShaderHash>,
    /// Shader readiness over the input set, in basis points (`cached_ok/total`).
    pub readiness_bps: u16,
    /// Readiness over the hot-hinted subset, in basis points.
    pub hot_readiness_bps: u16,
}

impl ShaderAotReport {
    /// Shader readiness as a percentage in `[0, 100]`.
    #[must_use]
    pub fn readiness_percent(&self) -> f32 {
        self.readiness_bps as f32 / 100.0
    }
}

/// Translate and cache a title's shader set, reporting shader readiness.
///
/// Hot-hinted shaders are compiled first (so a shared profile demonstrably
/// steers the order); remaining shaders keep their declared order. A per-shader
/// cache-insert failure is recorded and the pass continues — only genuine cache
/// I/O errors while pre-checking abort the pass.
pub fn compile_shaders(
    inputs: &[ShaderAotInput],
    hints: &ShaderProfileHints,
    cache: &ShaderCache,
) -> Result<ShaderAotReport, ShaderCacheError> {
    // Hot-hinted shaders first, otherwise stable in declared order.
    let mut order: Vec<usize> = (0..inputs.len()).collect();
    order.sort_by_key(|&index| !hints.is_hot(inputs[index].source_hash()));

    let mut report = ShaderAotReport {
        total: inputs.len(),
        ..ShaderAotReport::default()
    };

    for &index in &order {
        let input = &inputs[index];
        let hash = input.source_hash();
        let hot = hints.is_hot(hash);
        if hot {
            report.hot_total += 1;
        }
        report.order.push(hash);

        // Reuse an already-valid object rather than re-translating it.
        if matches!(cache.full_check(hash)?, ShaderCheckOutcome::Ok) {
            report.already_cached += 1;
            report.cached_ok += 1;
            if hot {
                report.hot_cached_ok += 1;
            }
            continue;
        }

        let object = translate(input.stage, &input.source, &input.entry).to_object();
        match cache.insert(&object) {
            Ok(_entry) => {
                report.compiled += 1;
                report.cached_ok += 1;
                if hot {
                    report.hot_cached_ok += 1;
                }
            }
            Err(error) => {
                report.failed += 1;
                report.errors.push(format!("{hash}: {error}"));
            }
        }
    }

    report.readiness_bps = bps(report.cached_ok, report.total);
    report.hot_readiness_bps = bps(report.hot_cached_ok, report.hot_total);
    Ok(report)
}

/// Shader readiness over a fixed set of known shaders: the fraction that are
/// present and valid in `cache`, in basis points. Independent of any one AOT
/// pass, so callers can report readiness before/after compilation.
pub fn cached_readiness_bps(
    cache: &ShaderCache,
    known: &[ShaderHash],
) -> Result<u16, ShaderCacheError> {
    let mut ready = 0usize;
    for &hash in known {
        if matches!(cache.full_check(hash)?, ShaderCheckOutcome::Ok) {
            ready += 1;
        }
    }
    Ok(bps(ready, known.len()))
}

/// Coverage in basis points, saturating at 10_000 (100.00%). Mirrors
/// `nx86-backend::coverage_bps`.
fn bps(numerator: usize, denominator: usize) -> u16 {
    if denominator == 0 {
        return 0;
    }
    (((numerator as u128) * 10_000) / (denominator as u128)).min(10_000) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(stage: ShaderStage, source: &[u8]) -> ShaderAotInput {
        ShaderAotInput::new(stage, source.to_vec(), "main")
    }

    fn temp_cache() -> (tempfile::TempDir, ShaderCache) {
        let dir = tempfile::tempdir().expect("tempdir");
        let cache = ShaderCache::open(dir.path()).expect("open cache");
        (dir, cache)
    }

    #[test]
    fn compiles_and_caches_the_whole_set() {
        let (_dir, cache) = temp_cache();
        let inputs = [
            input(ShaderStage::Vertex, b"void vert() {}"),
            input(ShaderStage::Fragment, b"void frag() {}"),
            input(ShaderStage::Compute, b"void comp() {}"),
        ];
        let report =
            compile_shaders(&inputs, &ShaderProfileHints::new(), &cache).expect("aot pass");
        assert_eq!(report.total, 3);
        assert_eq!(report.compiled, 3);
        assert_eq!(report.cached_ok, 3);
        assert_eq!(report.failed, 0);
        assert_eq!(report.readiness_bps, 10_000);
        // Every object is present and valid in the cache.
        for input in &inputs {
            assert!(matches!(
                cache.full_check(input.source_hash()).expect("check"),
                ShaderCheckOutcome::Ok
            ));
        }
    }

    #[test]
    fn second_pass_reuses_cached_objects() {
        let (_dir, cache) = temp_cache();
        let inputs = [input(ShaderStage::Vertex, b"void vert() {}")];
        let first = compile_shaders(&inputs, &ShaderProfileHints::new(), &cache).expect("first");
        assert_eq!(first.compiled, 1);
        assert_eq!(first.already_cached, 0);
        let second = compile_shaders(&inputs, &ShaderProfileHints::new(), &cache).expect("second");
        assert_eq!(second.compiled, 0);
        assert_eq!(second.already_cached, 1);
        assert_eq!(second.cached_ok, 1);
        assert_eq!(second.readiness_bps, 10_000);
    }

    #[test]
    fn hot_hinted_shaders_are_compiled_first() {
        let (_dir, cache) = temp_cache();
        let inputs = [
            input(ShaderStage::Vertex, b"void vert() {}"),
            input(ShaderStage::Fragment, b"void frag() {}"),
            input(ShaderStage::Compute, b"void comp() {}"),
        ];
        // Hint only the compute shader hot; it must lead the processing order.
        let hot_hash = inputs[2].source_hash();
        let hints = ShaderProfileHints::from_entries(vec![ShaderHint {
            source_hash: hot_hash,
            stage: ShaderStage::Compute,
            hot: true,
        }]);
        let report = compile_shaders(&inputs, &hints, &cache).expect("aot pass");
        assert_eq!(report.order.first(), Some(&hot_hash));
        assert_eq!(report.hot_total, 1);
        assert_eq!(report.hot_cached_ok, 1);
        assert_eq!(report.hot_readiness_bps, 10_000);
    }

    #[test]
    fn readiness_reflects_a_withheld_shader() {
        let (_dir, cache) = temp_cache();
        let full = [
            input(ShaderStage::Vertex, b"void vert() {}"),
            input(ShaderStage::Fragment, b"void frag() {}"),
            input(ShaderStage::Compute, b"void comp() {}"),
        ];
        let known: Vec<ShaderHash> = full.iter().map(ShaderAotInput::source_hash).collect();
        // Before compiling anything, nothing is ready.
        assert_eq!(cached_readiness_bps(&cache, &known).expect("before"), 0);

        // Compile only two of the three; readiness over the known set is 2/3.
        compile_shaders(&full[..2], &ShaderProfileHints::new(), &cache).expect("partial");
        assert_eq!(
            cached_readiness_bps(&cache, &known).expect("partial"),
            6_666
        );

        // Compiling the rest brings the set to full readiness.
        compile_shaders(&full, &ShaderProfileHints::new(), &cache).expect("complete");
        assert_eq!(cached_readiness_bps(&cache, &known).expect("full"), 10_000);
    }

    #[test]
    fn empty_set_is_zero_readiness() {
        let (_dir, cache) = temp_cache();
        let report = compile_shaders(&[], &ShaderProfileHints::new(), &cache).expect("empty");
        assert_eq!(report.total, 0);
        assert_eq!(report.readiness_bps, 0);
        assert_eq!(cached_readiness_bps(&cache, &[]).expect("none"), 0);
    }

    #[test]
    fn hints_version_validates() {
        let mut hints = ShaderProfileHints::new();
        assert!(hints.validate_version().is_ok());
        hints.format_version = 999;
        assert!(hints.validate_version().is_err());
    }
}
