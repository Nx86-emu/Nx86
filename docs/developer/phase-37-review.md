# Phase 37 Review: Scheduler Replay v0

Date: 2026-06-25
Reviewer: Codex

## What landed

| Crate | What |
|-------|------|
| `nx86-scheduler/src/lib.rs` | bounded `ReplayLog`, `ReplayEvent`, `ReplayMetadata`, crash-window filtering, `ReplayAnalysis` |
| `nx86-gui/src/screens.rs` | replay retained/dropped event metadata in the Scheduler screen |

## Findings

### FINDING-1: replay is in-memory only (info)

The replay window is intentionally not persisted. Crash files and long-running
trace storage remain out of scope until the runtime has real guest-thread
execution and service integration.

## Test coverage

| Test | What |
|------|------|
| `replay_log_keeps_bounded_crash_window_metadata` | cap enforcement, dropped count, crash event, and per-thread crash analysis |

## Verification

```
cargo test -p nx86-scheduler --lib -> PASS
```
