# Phase 36 Review: Guest Threading v0

Date: 2026-06-25
Reviewer: Codex

## What landed

| Crate | What |
|-------|------|
| `nx86-scheduler/src/lib.rs` | `Scheduler`, guest thread records, host mapping metadata, per-thread CPU ticks, synthetic multi-thread runner |
| `nx86-gui/src/screens.rs` | Scheduler screen rows for guest thread status, PC, host mapping, fiber slot, and CPU ticks |
| `nx86-core/src/config.rs` | `AppScreen::Scheduler` navigation entry |

## Findings

### FINDING-1: thread execution is synthetic only (info)

Phase 36 intentionally models guest threads and accounting without spawning real
host execution workers. This keeps the phase deterministic and leaves service
thread integration for later runtime/HLE phases.

## Test coverage

| Test | What |
|------|------|
| `synthetic_multi_thread_program_runs_on_host_threads` | two guest threads run to halt with deterministic host indexes and CPU ticks |
| GUI compile coverage | the Scheduler screen consumes `ThreadGuiRow` from `nx86-scheduler` |

## Verification

```
cargo test -p nx86-scheduler --lib -> PASS
```
