# Phase 43: Native Coverage System

Phase 43 turns Native Coverage from one rough progress number into explicit
functional, static, executed, and memory-path metrics.

## What it does

- **Functional coverage metric** - `CoverageSnapshot::functional_coverage_bps`
  reports promoted native blocks over profile JIT candidates.
- **Static coverage metric** - `static_coverage_bps` reports cached native
  blocks over all source blocks in the function.
- **Executed coverage metric** - `executed_coverage_bps` reports cached native
  blocks over blocks observed in the runtime profile.
- **Fastmem coverage** - profiles can record `Fastmem` events; coverage reports
  `fastmem_calls` and `fastmem_coverage_bps`.
- **Slowmem penalty** - coverage reports `slowmem_calls` and
  `slowmem_penalty_bps` over profiled memory accesses.
- **GUI display** - compile progress now carries and renders Native Coverage,
  Static Native, Executed Native, Fastmem, and Slowmem Penalty metrics.

## Design

Coverage percentages are stored as basis points in backend snapshots to keep
reporting deterministic and equality-testable. IPC converts them to display
percentages for the GUI. Existing `native_coverage_estimate` remains as the
compact headline value.

## Phase boundary

Phase 43 owns reporting and GUI display for current rebuild/profile data. Future
phases can add lower-overhead fastmem sampling in generated native code and
persist richer coverage history per title.
