# Phase 25 Review

Date: 2026-06-21

## Scope

This review covers Phase 25 from `SPEC.md`: reading a runtime profile,
identifying JIT block candidates, promoting them to AOT objects, and reporting
native coverage metrics.

## Findings

- `ProfileLog::jit_block_candidates()` borrows from the profile records and
  returns only `JitBlock` events with their `guest_pc`, `code_size_bytes`, and
  `cache_file_name`.
- `rebuild_from_profile` iterates candidates through `EmergencyJit::compile`,
  the same trusted path used by the runtime dispatcher. Promoted objects are
  inserted into the cache; unknown PCs and errors are counted but do not stop
  the pass.
- The rebuild is idempotent: re-running with the same profile overwrites cached
  objects with identical content (deterministic recompilation).
- `RebuildOutcome` reports per-candidate breakdown and a `CoverageSnapshot`
  with post-rebuild native block count and remaining JIT fallback estimate.
- `WorkerKind::RebuildProfile` extends the IPC model for future worker-driven
  rebuild without requiring GUI integration in this phase. The CLI smoke path
  reports seven phases: read-profile, identify, recompile, insert, coverage,
  verify, report.
- `nx86-cache` was promoted from a dev-dependency to a regular dependency of
  `nx86-backend`, since the rebuild function scans the cache to report total
  native block counts.

## Boundary Checks

- Rebuild candidates are identified by guest address only; no guest bytes,
  memory contents, or personal data are read from the profile.
- The rebuild uses the same `EmergencyJit` verification and cache insertion
  path as runtime JIT; no new code-generation or unsafe boundaries are
  introduced.
- Coverage metrics are derived from profile and cache counts; they do not
  inspect generated code or guest memory.

## Verification

Passed locally on `aarch64-apple-darwin`:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets   # 163 tests, 0 failures
cargo build --workspace
```

The Linux-only integration test (`promoted_blocks_skip_jit_on_second_run`) is
compiled on the Apple Silicon development host but is not executed locally. It
runs a two-block function with only block 0 loaded, lets block 1 get
JIT-compiled and profiled, rebuilds from the profile, then confirms a second
dispatch run uses the promoted object and emits zero JIT events.
