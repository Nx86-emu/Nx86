# Phase 43 Review: Native Coverage System

Date: 2026-06-25
Reviewer: Codex

## What landed

| Crate | What |
|-------|------|
| `nx86-profile/src/lib.rs` | `Fastmem` profile event and validation |
| `nx86-jit/src/emergency.rs` | source block count and entry-PC accessors for coverage |
| `nx86-backend/src/lib.rs` | expanded `CoverageSnapshot` with functional/static/executed/fastmem/slowmem metrics |
| `nx86-core/src/ipc.rs` | compile progress fields for detailed Native Coverage display |
| `nx86-gui/src/screens.rs` | compile screen renders static, executed, fastmem, and slowmem coverage metrics |
| `nx86-app/src/main.rs` | smoke worker emits populated coverage fields |

## Findings

### FINDING-1: old coverage estimate collapsed different questions (fixed)

The previous rebuild snapshot exposed promoted/native counts and one compact
coverage estimate. Phase 43 now separates functional, static, executed, and
memory-path coverage.

### FINDING-2: fastmem had no profile event type (fixed)

Profiles can now carry typed `Fastmem` observations, allowing reports to compute
fastmem coverage and slowmem penalty over known memory accesses.

### FINDING-3: GUI needed more than the headline value (fixed)

Compile progress still shows the compact Native Coverage value, and now renders
Static Native, Executed Native, Fastmem, and Slowmem Penalty.

## Test coverage

| Test | What |
|------|------|
| `round_trips_every_event_type` | profile format includes `Fastmem` |
| `impossible_event_sizes_are_rejected` | invalid fastmem access sizes are rejected |
| `rebuild_from_profile_reports_native_coverage_metrics` | functional/static/executed/fastmem/slowmem metrics |
| `event_json_round_trips` | IPC progress serializes detailed coverage fields |

## Verification

```
cargo test -p nx86-profile -p nx86-backend -p nx86-core -p nx86-gui --lib -> PASS
```
