# Phase 49 Review: Shader Translation Skeleton

Date: 2026-06-27
Reviewer: Claude (Opus 4.8)

## What landed

| Crate | What |
|-------|------|
| `nx86-shader/src/lib.rs` | `ShaderStage`, `TranslationStatus`, `ShaderMetadata` (+ `validate_version`), `ShaderHash` (FNV-1a), and the deterministic `translate` placeholder path |
| `nx86-shader/src/object.rs` | `.nxshader` container (magic `NXS\0`, header, body, trailing FNV-1a hash) with `encode`/`decode` + validation, mirroring `nx86-object` |
| `nx86-shader/src/cache.rs` | `ShaderCache` mirroring `nx86-cache::CacheManager`: scan-as-truth, JSON manifest, shallow/full checks, size accounting, atomic writes, insert/load/remove/clear |
| `nx86-testsuite/src/lib.rs` | clean-room `SyntheticShader` spec (TOML, hex source, string `stage`) + `sample()` |
| `nx86-core/src/storage.rs` | `StorageLayout::shader_cache_dir(title_id)` helper |
| `nx86-app/src/main.rs` | compiler-smoke worker translates + caches the sample shader and reports it over JSON-line IPC |
| `nx86-gui/src` | Tests-screen "Translate & cache sample shader" control + status line |

## Findings

### FINDING-1: the shader stack deliberately mirrors the CPU AOT stack

`nx86-shader` is the GPU-side parallel of `nx86-object` + `nx86-cache`: the same
FNV-1a integrity trailer, the same scan-is-source-of-truth manifest, and the same
atomic-write cache surface. Reviewers familiar with the CPU cache get the shader
cache for free. The hash constants are re-implemented (not shared) because
`nx86-object::fnv1a` is private; both are the standard FNV-1a-64.

### FINDING-2: no `nx86-testsuite` → `nx86-shader` dependency (kept decoupled)

The synthetic shader's `stage` is a free-form string parsed into `ShaderStage` at
the boundary (the GUI/worker), exactly as `entry_point` is parsed for synthetic
ARM64 tests, so the test crate stays independent of the shader-model crate.

### FINDING-3: host-independent, legal boundary respected

No Vulkan, SPIR-V, or GPU execution: the translation is a deterministic
placeholder blob, and the synthetic input is opaque clean-room bytes (no real
game shaders). Everything compiles and tests on the Apple Silicon dev host.

### FINDING-4: Native Coverage intentionally untouched

Per SPEC, shader readiness contributing to Native Coverage is Phase 50. Phase 49
adds no coverage wiring; that boundary is explicit in `phase-49.md`.

## Test coverage

| Test | What |
|------|------|
| `nx86-shader::translate_is_deterministic_and_marks_placeholder` | translation determinism + status |
| `nx86-shader::shader_hash_is_deterministic_and_padded` | hash stability + hex width |
| `nx86-shader::object::*` | `.nxshader` round-trip, bad magic/version/truncation/corruption rejection |
| `nx86-shader::cache::*` | insert/load round-trip, shallow+full checks, scan/manifest, clear, remove |
| `nx86-testsuite::synthetic_shader_parses_stage_and_decodes_hex_source` | TOML spec parse |
| `nx86-core::shader_cache_dir_is_under_the_title_cache_folder` | storage helper |
| `nx86-gui::compile_sample_shader_populates_shader_status` | GUI wiring (translate + cache + integrity) |

## Verification

Host-independent (Apple Silicon dev host):

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo run -p nx86-app -- --worker compiler-smoke
```

All passed (33 test binaries green). The compiler-smoke worker reports:
`compiled vertex shader 'sample triangle vertex' to Placeholder placeholder,
cached 387047402e231c81.nxshader (56 bytes, check=Ok)` — satisfying the exit
criterion "synthetic shader path compiles/caches placeholder."
