# Phase 50 Review: Shader AOT v0

Date: 2026-06-27
Reviewer: Claude (Opus 4.8)

## What landed

| Crate | What |
|-------|------|
| `nx86-shader/src/aot.rs` | `ShaderProfileHints`/`ShaderHint`, `ShaderAotInput`, `compile_shaders` (hot-first batch translate+cache), `ShaderAotReport`, `cached_readiness_bps` |
| `nx86-core/src/coverage.rs` | `NativeCoverage` min-gate combiner (`combined_estimate_bps`), `CoverageBand` (§15.4 labels), full-scale clamping |
| `nx86-core/src/ipc.rs` | `CompileProgress.shader_readiness` field (serde default) |
| `nx86-testsuite/src/lib.rs` | `SyntheticShader::sample_set()` (clean-room vertex/fragment/compute), private `synthetic`/`encode_hex` helpers |
| `nx86-gui/src` | Compile-screen "Shader readiness" sub-metric + gating axis; Tests-screen "Compile & cache shader set" batch control |
| `nx86-app/src/main.rs` | compiler-smoke batch AOT reporting the gated Native Coverage moving 0% → 100% |

## Findings

### FINDING-1: shader readiness gates Native Coverage via a min-gate (exit criterion)

`NativeCoverage::combined_estimate_bps` = `min(cpu_functional, shader_readiness)`.
This satisfies the Phase 50 exit criterion ("shader AOT contributes to Native
Coverage") and SPEC §15.5 (Perfect requires both axes at 100%): a weak shader axis
caps the headline until it catches up. The combiner lives in `nx86-core` and takes
only basis-point integers, so it adds no dependency on `nx86-shader`/`nx86-backend`.

### FINDING-2: shared profile hints steer the pass without a hard dependency

`ShaderProfileHints` is a `format_version`'d serde set; `compile_shaders` sorts
hot-hinted shaders first (stable otherwise) and reports `hot_readiness_bps`. This
is the GPU parallel of the CPU `ProfileLog` → `rebuild_from_profile` flow, kept
optional (an empty hint set is valid and leaves declared order intact).

### FINDING-3: readiness is reported two ways, intentionally

`ShaderAotReport.readiness_bps` is readiness over the pass's own inputs;
`cached_readiness_bps(cache, known)` is readiness over a fixed known set, so a
caller can measure before/after a pass (the worker uses this to show 0% → 100%) or
account for withheld shaders (the `readiness_reflects_a_withheld_shader` test:
2/3 → 6_666 bps).

### FINDING-4: host-independent, legal boundary respected

No real SPIR-V/Maxwell, no GPU execution: translation stays the Phase 49
deterministic placeholder, and the shader set is opaque clean-room bytes. Pipeline
readiness and the Vulkan pipeline cache remain Phase 51+. Everything compiles and
tests on the Apple Silicon dev host.

## Test coverage

| Test | What |
|------|------|
| `nx86-shader::aot::compiles_and_caches_the_whole_set` | full set → readiness 100% |
| `nx86-shader::aot::second_pass_reuses_cached_objects` | idempotent re-run (already_cached) |
| `nx86-shader::aot::hot_hinted_shaders_are_compiled_first` | hint-driven ordering |
| `nx86-shader::aot::readiness_reflects_a_withheld_shader` | 0% → 2/3 → 100% over a known set |
| `nx86-shader::aot::empty_set_is_zero_readiness` | denominator-zero safety |
| `nx86-core::coverage::*` | min-gate, Perfect-requires-both, §15.4 band boundaries, input clamp |
| `nx86-core::ipc::event_json_round_trips` | `shader_readiness` round-trips |
| `nx86-testsuite::synthetic_shader_sample_set_spans_distinct_stages` | sample set parses/decodes, distinct |
| `nx86-gui::compile_shader_set_populates_shader_status` | batch action → readiness + gated band |

## Verification

Host-independent (Apple Silicon dev host):

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo run -p nx86-app -- --worker compiler-smoke
```

All passed (65 test binaries green). The compiler-smoke worker reports:
`shader AOT: compiled 3/3 shaders (readiness 100.00%, hot 1/1); Native Coverage
0.00% [Terrible] -> 100.00% [Perfect] (CPU 100% assumed)` — satisfying the exit
criterion "shader AOT contributes to Native Coverage."
