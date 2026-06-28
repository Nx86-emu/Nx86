# Phase 50: Shader AOT v0

Phase 50 turns the Phase 49 single-shader primitives into the first real **Shader
AOT** slice: a batch pass that compiles a title's whole shader set during initial
compile, driven by shared profile hints, and — the defining step — folds **shader
readiness into Native Coverage**. It is the GPU-side parallel of how
`nx86-backend::rebuild_from_profile` drives CPU coverage.

## What it does

- **Compile shaders during initial compile** — `nx86-shader::compile_shaders`
  translates every shader in a set to a placeholder (Phase 49 `translate`), caches
  each as a `.nxshader` object, and tolerates per-shader failure (recorded, never
  panics). It returns a `ShaderAotReport` (`total`, `compiled`, `already_cached`,
  `cached_ok`, `failed`, hot counts, `readiness_bps`, `hot_readiness_bps`).
- **Store shader cache** — reuses the Phase 49 `ShaderCache`; a second pass reuses
  already-valid objects instead of re-translating them.
- **Use shared profile hints** — `ShaderProfileHints` (a `format_version`'d serde
  set of `ShaderHint { source_hash, stage, hot }`) is the "shared profiles MAY
  assist shader AOT" seam (SPEC §33.2). Hot-hinted shaders are compiled first, so
  a shared profile demonstrably steers the pass — the GPU analogue of the CPU
  `ProfileLog`.
- **Report shader readiness** — readiness is `cached_ok / total` in basis points;
  `cached_readiness_bps` reports readiness over a fixed known set independent of any
  one pass (so callers can compare before/after).
- **Contribute to Native Coverage** — `nx86-core::coverage::NativeCoverage` is the
  single source of truth for how the axes combine. It takes already-computed
  basis-point inputs (CPU functional + shader readiness — so it needs no dependency
  on `nx86-backend` or `nx86-shader`) and combines them with a **min-gate**:
  `combined = min(cpu, shader)`. `CoverageBand::from_bps` classifies the headline
  into the SPEC §15.4 bands (Terrible/Poor/Great/Excellent/Perfect).
- **Surface it** — `CompileProgress` gains a `shader_readiness` field; the GUI
  Compile screen shows a "Shader readiness" sub-metric and which axis gates the
  headline; the Tests screen's shader control runs the batch pass over the sample
  set and reports readiness + the gated Native Coverage.
- **Demonstration** — the compiler-smoke worker compiles the clean-room
  `SyntheticShader::sample_set()` (one shader hinted hot) into a temp cache and
  reports the headline moving from `0.00% [Terrible]` (nothing cached) to
  `100.00% [Perfect]` (full set cached, CPU assumed full) — concrete evidence that
  shader AOT contributes to Native Coverage.

## Design

The min-gate is faithful to SPEC §15.5: "Perfect" requires *both* 100% CPU
readiness and 100% shader/pipeline readiness, so the weakest axis caps the
combined number until it catches up. A single user-facing percentage folds GPU
readiness in (SPEC §33.3) rather than showing a separate GPU number; developer
sub-metrics (shader readiness, the gating axis) remain visible.

The combiner lives in `nx86-core` because that crate already owns the IPC
`native_coverage_*` fields and shared semantics, and a combiner over basis-point
integers introduces no new dependency. The shader set is a clean-room sample in
`nx86-testsuite` (`sample_set()`), keeping the synthetic inputs alongside the
existing ARM64/framebuffer specs; stages stay strings parsed at the boundary, so
`nx86-testsuite` keeps no dependency on `nx86-shader`.

This is host-independent: pure logic plus `std` file I/O, no Vulkan/SPIR-V/GPU
execution. It compiles and tests fully on the Apple Silicon dev host and in
headless CI.

## Phase boundary

Phase 50 still does not generate real SPIR-V or translate real (Maxwell/NVN)
shader binaries, and does not run shaders on the GPU — translation remains the
deterministic placeholder. Only the **shader** axis folds into Native Coverage
here; **pipeline** readiness and the Vulkan pipeline cache are Phase 51+. The CPU
functional axis used in the demonstrations is a representative value, not yet wired
to a live `CoverageSnapshot`.
