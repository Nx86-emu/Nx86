# Phase 49: Shader Translation Skeleton

Phase 49 stands up the GPU-side parallel of the CPU AOT object/cache stack: a way
to model a shader, hash its source, "translate" it, and cache the result. It is
the foundation Phase 50 (Shader AOT v0) builds on, where shader readiness starts
contributing to Native Coverage.

## What it does

- **Shader metadata model** - `nx86-shader` defines `ShaderStage`
  (vertex/fragment/compute), `TranslationStatus`, and `ShaderMetadata`
  (`format_version`, stage, source hash, entry, source/translated lengths,
  status), with a `validate_version` check following the
  `PROFILE_FORMAT_VERSION` convention.
- **Shader hash model** - `ShaderHash` is an FNV-1a-64 of the source bytes, the
  same dependency-free hashing the `.nxo` objects use, formatted `{:016x}` for
  on-disk names.
- **Placeholder translation path** - `translate(stage, source, entry)` produces a
  deterministic placeholder blob (a tag + stage code + source hash), **not real
  SPIR-V**. The seam exists so Phase 50 can drop in a real compiler while the
  hashing, container, and cache around it stay put.
- **Shader cache folder** - `.nxshader` is a self-describing, integrity-checked
  binary object (mirrors `.nxo`: magic `NXS\0`, header, body, trailing FNV-1a
  hash). `ShaderCache` mirrors `nx86-cache`'s `CacheManager`: scan-as-source-of-
  truth, JSON manifest, shallow/full integrity checks, size accounting, atomic
  writes, and insert/load/remove/clear. It lives in the title's `cache/shaders/`
  folder (`StorageLayout::shader_cache_dir`, already in `REQUIRED_TITLE_DIRS`),
  kept separate from the CPU cache per SPEC §33.2.
- **Synthetic shader input** - `nx86-testsuite` gains a clean-room `SyntheticShader`
  spec (TOML, hex source, string `stage`) alongside the synthetic ARM64 test and
  framebuffer specs, plus a built-in `sample()`.
- **Demonstration** - the compiler-smoke worker (SPEC §14.1 "compile shaders
  during initial compile") translates + caches the sample shader and reports it
  over the versioned JSON-line IPC; the GUI Tests screen gains a "Translate &
  cache sample shader" control showing stage, status, cached file, and integrity.

## Design

The shader stack is a deliberate mirror of the CPU AOT stack so it is immediately
familiar: `nx86-shader` is to the GPU what `nx86-object` + `nx86-cache` are to the
CPU, down to the FNV-1a integrity trailer, the scan-is-truth manifest, and the
atomic-write cache surface.

The synthetic shader's `stage` is a free-form string parsed into `ShaderStage` at
the boundary (exactly as `entry_point` is parsed for synthetic ARM64 tests), so
`nx86-testsuite` needs no dependency on `nx86-shader`. The legal boundary forbids
real game shaders, so the synthetic inputs are opaque placeholder bytes only.

This is host-independent: no Vulkan, no SPIR-V, no GPU execution. It compiles and
tests fully on the Apple Silicon dev host and in headless CI.

## Phase boundary

Phase 49 does not generate real SPIR-V or translate real (Maxwell/NVN) shader
binaries, does not run shaders on the GPU, and **does not wire shader readiness
into Native Coverage** — that is Phase 50 ("shader AOT contributes to Native
Coverage"). Pipeline objects and the Vulkan pipeline cache are Phase 51+.
