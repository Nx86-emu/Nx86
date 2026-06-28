# Phase 51: Vulkan Pipeline Cache Integration

Phase 51 adds the pipeline cache infrastructure that wraps Vulkan's
`VkPipelineCache` with save/load persistence and integrates pipeline readiness
into the Native Coverage min-gate alongside CPU and shader readiness.

## What it does

- **Pipeline profile hints** — `PipelineProfileHints` in `nx86-shader` is a
  versioned, serde-serializable set of `PipelineHint`s (mirroring
  `ShaderProfileHints`). Each hint identifies a pipeline by `PipelineKey` (a
  deterministic FNV-1a hash of shader combination + render state) and whether
  it was observed hot. The format is the "shared profiles MAY assist pipeline
  cache warming" seam.

- **Pipeline cache blob** — `PipelineCacheBlob` in `nx86-vulkan` owns the
  opaque pipeline cache bytes and persists them to `cache/pipelines/
  pipeline-cache.bin` with atomic writes (temp file + rename). It tracks a
  dirty flag so `Drop` can best-effort save. Higher-level code passes the blob
  to `vkCreatePipelineCache` as `initial_data` and retrieves it via
  `vkGetPipelineCacheData`.

- **Pipeline miss log** — `PipelineMissLog` in `nx86-vulkan` is an append-only
  JSONL file for recording pipeline cache misses at runtime. The compiler worker
  reads this log on the next compile cycle to prioritize missing pipelines.
  Infrastructure is created but not yet called (no runtime pipeline creation
  until Phase 52+).

- **Pipeline readiness in Native Coverage** — `NativeCoverage` in `nx86-core`
  gains a `pipeline_readiness_bps` axis. The min-gate now covers three axes:
  CPU, shader, and pipeline. `PipelineProfileHints::new()` defaults pipeline
  readiness to full scale (10_000 bps) so the two-axis behavior is preserved
  when pipeline readiness is not explicitly set. `CompileProgress` gains a
  `pipeline_readiness` field.

- **Storage layout** — `StorageLayout::pipeline_cache_dir` returns the
  per-title `cache/pipelines/` directory (already in `REQUIRED_TITLE_DIRS`).

## Design

The pipeline cache follows the same pattern as the shader cache: a per-title
directory, a blob, manifest/metadata, and integration with the AOT pass. The
`PipelineKey` is derived from shader hashes + state descriptor using FNV-1a,
matching the hashing convention used by `.nxo` and `.nxshader` objects.

The three-axis min-gate is faithful to SPEC §15.5: "Perfect" requires all axes
at 100%. The `new()` constructor defaults `pipeline_readiness_bps` to full
scale so existing two-axis callers are unaffected; `with_pipeline()` explicitly
sets the pipeline axis.

This is host-independent: pure logic plus `std` file I/O, no Vulkan device
required. It compiles and tests fully on the Apple Silicon dev host and in
headless CI.

## Phase boundary

Phase 51 creates the pipeline cache infrastructure but does not yet create real
Vulkan pipelines or call `vkCreateGraphicsPipelines`. The blob starts empty;
the first real pipeline creation populates it (Phase 52+). The miss log
infrastructure is created but not yet called from runtime code. The "initial
compile integration" goal is satisfied by the `PipelineCacheBlob::open` +
`save` lifecycle; actual pipeline compilation through the cache is Phase 52.
