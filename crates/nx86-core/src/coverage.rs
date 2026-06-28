//! Combined Native Coverage (Phase 50).
//!
//! Native Coverage is the project's main progress metric (SPEC §15). It folds
//! several axes — CPU readiness, shader/pipeline readiness, fastmem, … — into a
//! single user-facing percentage that represents functional readiness toward
//! Pure AOT (§15.3). GPU/shader readiness is *included* in that one number, not
//! shown as a separate GPU percentage (§33.3).
//!
//! This module is the single source of truth for **how the axes combine**. It
//! operates purely on already-computed basis-point inputs (so it needs no
//! dependency on `nx86-backend` or `nx86-shader`) and is host-independent.
//!
//! Phase 50 combines the CPU functional axis with the shader-readiness axis with
//! a **min-gate**: the headline can be no higher than the weakest axis. This is
//! faithful to §15.5 — "Perfect" requires *both* 100% CPU readiness and 100%
//! shader/pipeline readiness, so a weak axis caps the combined number until it
//! catches up.

use serde::{Deserialize, Serialize};

/// Full scale for a basis-point coverage value: 10_000 bps = 100.00%.
pub const COVERAGE_FULL_BPS: u16 = 10_000;

/// The §15.4 coverage band a percentage falls into.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CoverageBand {
    Terrible,
    Poor,
    Great,
    Excellent,
    Perfect,
}

impl CoverageBand {
    /// Classify a basis-point coverage value per the SPEC §15.4 bands.
    #[must_use]
    pub const fn from_bps(bps: u16) -> Self {
        match bps {
            10_000.. => Self::Perfect,
            9_800..=9_999 => Self::Excellent,
            9_000..=9_799 => Self::Great,
            6_000..=8_999 => Self::Poor,
            _ => Self::Terrible,
        }
    }

    /// Human-readable band label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Terrible => "Terrible",
            Self::Poor => "Poor",
            Self::Great => "Great",
            Self::Excellent => "Excellent",
            Self::Perfect => "Perfect",
        }
    }
}

/// The axes that fold into the combined Native Coverage headline.
///
/// Inputs are basis points (`0..=10_000`); values above full scale are clamped.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct NativeCoverage {
    /// CPU functional native coverage, in basis points.
    pub cpu_functional_bps: u16,
    /// Shader readiness, in basis points.
    pub shader_readiness_bps: u16,
    /// Pipeline readiness, in basis points.
    pub pipeline_readiness_bps: u16,
}

impl NativeCoverage {
    #[must_use]
    pub fn new(cpu_functional_bps: u16, shader_readiness_bps: u16) -> Self {
        Self {
            cpu_functional_bps: cpu_functional_bps.min(COVERAGE_FULL_BPS),
            shader_readiness_bps: shader_readiness_bps.min(COVERAGE_FULL_BPS),
            pipeline_readiness_bps: COVERAGE_FULL_BPS,
        }
    }

    #[must_use]
    pub fn with_pipeline(mut self, pipeline_readiness_bps: u16) -> Self {
        self.pipeline_readiness_bps = pipeline_readiness_bps.min(COVERAGE_FULL_BPS);
        self
    }

    /// The combined headline, in basis points. Min-gate: the weakest axis caps
    /// the result (so it reaches full scale only when all axes do).
    #[must_use]
    pub const fn combined_estimate_bps(self) -> u16 {
        let mut min = self.cpu_functional_bps;
        if self.shader_readiness_bps < min {
            min = self.shader_readiness_bps;
        }
        if self.pipeline_readiness_bps < min {
            min = self.pipeline_readiness_bps;
        }
        min
    }

    /// The combined headline as a percentage in `[0, 100]`.
    #[must_use]
    pub fn combined_percent(self) -> f32 {
        self.combined_estimate_bps() as f32 / 100.0
    }

    #[must_use]
    pub fn cpu_functional_percent(self) -> f32 {
        self.cpu_functional_bps as f32 / 100.0
    }

    #[must_use]
    pub fn shader_readiness_percent(self) -> f32 {
        self.shader_readiness_bps as f32 / 100.0
    }

    #[must_use]
    pub fn pipeline_readiness_percent(self) -> f32 {
        self.pipeline_readiness_bps as f32 / 100.0
    }

    /// The §15.4 band of the combined headline.
    #[must_use]
    pub const fn band(self) -> CoverageBand {
        CoverageBand::from_bps(self.combined_estimate_bps())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn min_gate_caps_at_the_weakest_axis() {
        let coverage = NativeCoverage::new(8_000, 6_000);
        assert_eq!(coverage.combined_estimate_bps(), 6_000);
        assert_eq!(coverage.combined_percent(), 60.0);
        assert_eq!(coverage.band(), CoverageBand::Poor);

        // Strengthening the weak axis lifts the headline.
        let stronger = NativeCoverage::new(8_000, 9_500);
        assert_eq!(stronger.combined_estimate_bps(), 8_000);
        assert_eq!(stronger.band(), CoverageBand::Poor);
    }

    #[test]
    fn perfect_requires_both_axes_full() {
        assert_eq!(
            NativeCoverage::new(10_000, 10_000).band(),
            CoverageBand::Perfect
        );
        // One axis short of full is not Perfect, even if the other is maxed.
        assert_ne!(
            NativeCoverage::new(10_000, 9_999).band(),
            CoverageBand::Perfect
        );
        assert_eq!(NativeCoverage::new(10_000, 0).combined_estimate_bps(), 0);
    }

    #[test]
    fn band_boundaries_match_spec() {
        assert_eq!(CoverageBand::from_bps(0), CoverageBand::Terrible);
        assert_eq!(CoverageBand::from_bps(5_999), CoverageBand::Terrible);
        assert_eq!(CoverageBand::from_bps(6_000), CoverageBand::Poor);
        assert_eq!(CoverageBand::from_bps(8_999), CoverageBand::Poor);
        assert_eq!(CoverageBand::from_bps(9_000), CoverageBand::Great);
        assert_eq!(CoverageBand::from_bps(9_799), CoverageBand::Great);
        assert_eq!(CoverageBand::from_bps(9_800), CoverageBand::Excellent);
        assert_eq!(CoverageBand::from_bps(9_999), CoverageBand::Excellent);
        assert_eq!(CoverageBand::from_bps(10_000), CoverageBand::Perfect);
    }

    #[test]
    fn inputs_clamp_to_full_scale() {
        let coverage = NativeCoverage::new(12_000, 11_000);
        assert_eq!(coverage.cpu_functional_bps, COVERAGE_FULL_BPS);
        assert_eq!(coverage.shader_readiness_bps, COVERAGE_FULL_BPS);
        assert_eq!(coverage.band(), CoverageBand::Perfect);
    }

    #[test]
    fn three_axis_min_gate() {
        let coverage = NativeCoverage::new(9_000, 8_000).with_pipeline(7_000);
        assert_eq!(coverage.combined_estimate_bps(), 7_000);
        assert_eq!(coverage.band(), CoverageBand::Poor);

        // All three at full = Perfect.
        let full = NativeCoverage::new(10_000, 10_000).with_pipeline(10_000);
        assert_eq!(full.band(), CoverageBand::Perfect);

        // Pipeline axis short of full is not Perfect.
        let partial = NativeCoverage::new(10_000, 10_000).with_pipeline(9_999);
        assert_ne!(partial.band(), CoverageBand::Perfect);
    }
}
