# Phase 25: Profile-Guided Rebuild v0

Phase 25 closes the Continuous Dynamic Compilation loop: runtime profile
observations collected by Phase 24 are consumed to promote JIT-compiled blocks
into the persistent AOT cache, so a second execution run uses promoted native
objects without emergency JIT.

## What it does

`ProfileLog::jit_block_candidates()` filters the profile to `JitBlock` events,
each carrying the `guest_pc` needed to recompile the block through the existing
AOT pipeline.

`nx86_backend::rebuild_from_profile` iterates those candidates and calls
`EmergencyJit::compile` for each one — the same trusted path the dispatcher
uses at runtime, but driven offline by profile data rather than by a missing
block at dispatch time. Successfully compiled blocks are inserted into the
`CacheManager`; unknown PCs and compilation failures are counted but do not
stop the pass.

A `RebuildOutcome` reports promoted, skipped, and errored counts plus a
`CoverageSnapshot` with the post-rebuild native block count and remaining JIT
fallback estimate. This feeds `CompileProgress::native_coverage_estimate` in
the worker IPC model.

`WorkerKind::RebuildProfile` extends the IPC worker model for future GUI/CLI
integration. Phase 25 exercises the rebuild through direct function calls and
synthetic tests; full GUI integration is a later phase.

## New types

- `JitBlockCandidate<'a>` — borrows from a profile record, carries `guest_pc`,
  `code_size_bytes`, and `cache_file_name`
- `RebuildOutcome` — per-candidate breakdown: total, promoted, skipped, errors
- `CoverageSnapshot` — post-rebuild metrics: promoted blocks, total native
  blocks, remaining JIT fallbacks
- `RebuildError` — wraps cache scan failures

## Exit Criteria

- `jit_block_candidates()` extracts only `JitBlock` events from a mixed profile.
- `rebuild_from_profile` promotes JIT blocks into the cache with typed outcome.
- On Linux x86_64, a second dispatch run uses promoted objects and emits no JIT
  event.
- Unknown profile PCs and compilation failures are counted without stopping the
  pass.
- Coverage snapshot reflects the post-rebuild cache state.
