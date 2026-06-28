# Phase 51 Review

## Summary

Phase 51 adds Vulkan pipeline cache infrastructure: profile hints, blob
persistence, miss logging, and pipeline readiness in Native Coverage.

## What landed

### Pipeline profile hints (`nx86-shader`)
- `PipelineKey(u64)` — deterministic FNV-1a hash of shader hashes + state
- `PipelineHint { pipeline_key, hot }` — one hint entry
- `PipelineProfileHints { format_version, entries }` — versioned, serde
- Tests: determinism, version validation, hot lookup, serde round-trip

### Pipeline cache blob (`nx86-vulkan`)
- `PipelineCacheBlob` — owns opaque blob bytes, save/load/round-trip
- `PipelineCacheError` — typed error with I/O and size limit variants
- `PipelineMissLog` — append-only JSONL for runtime miss recording
- `MAX_PIPELINE_CACHE_BLOB` = 64 MiB safety limit
- Tests: open creates empty, save/reload round-trips, miss log records

### Native Coverage three-axis min-gate (`nx86-core`)
- `NativeCoverage::pipeline_readiness_bps` — third axis (defaults to full scale)
- `NativeCoverage::with_pipeline()` — explicit setter
- `combined_estimate_bps` — now min over CPU, shader, pipeline
- `pipeline_readiness` field on `CompileProgress`
- `pipeline_cache_dir` method on `StorageLayout`
- Tests: three-axis min-gate, Perfect requires all three

### Dependencies added
- `thiserror` to `nx86-vulkan` (for `PipelineCacheError`)
- `tempfile` as dev-dependency to `nx86-vulkan` (for tests)

## Test results

343 tests passing (7 new). Full workspace: fmt clean, clippy `-D warnings` clean.

## Exit criterion

Pipeline cache infrastructure persists across runs (blob save/load round-trips).
Actual pipeline creation through the cache is deferred to Phase 52.

## Boundary notes

- No real Vulkan pipeline creation — blob starts empty
- No runtime miss log calls — infrastructure only
- Pipeline readiness defaults to full scale so existing behavior is preserved
- `PipelineKey` derivation uses FNV-1a, matching project hashing conventions
