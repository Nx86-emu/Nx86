# Phase 45 Review: HLE/Service Skeleton

Date: 2026-06-26
Reviewer: Codex

## What landed

| Crate | What |
|-------|------|
| `nx86-hle/src/lib.rs` | typed SVC request, service dispatcher, filesystem/thread/memory/input skeleton outcomes |
| `nx86-runtime/src/lib.rs` | homebrew service handler path that continues after handled services and records service events |
| `nx86-input/src/lib.rs` | neutral input snapshot primitive for the input service skeleton |

## Findings

### FINDING-1: homebrew SVC handling stopped at the first call (fixed)

Phase 44 classified the first halted SVC after interpretation. That was enough
for `svc #0` exit, but not for a program that calls services before exiting.
The interpreter now has a private handler-enabled path; the public/default path
still halts on SVC, while `boot_homebrew` can handle known skeleton services and
continue.

### FINDING-2: service results needed a stable register convention (fixed)

Handled skeleton services return `x0 = 0` for success and `x1` for the service
payload. The payload is deterministic: current synthetic thread id, 4 KiB page
size, or neutral input state depending on the service.

### FINDING-3: unknown services must remain visible (fixed)

Unknown SVC immediates still halt homebrew execution and are reported as
`SvcOutcome::Unhandled`, so later HLE work can distinguish unsupported services
from successful stubs.

## Test coverage

| Test | What |
|------|------|
| `filesystem_service_returns_success_stub` | filesystem skeleton status |
| `thread_service_reports_current_synthetic_thread` | thread skeleton payload |
| `memory_service_reports_synthetic_page_size` | memory skeleton payload |
| `input_service_returns_neutral_controller_state` | input skeleton payload |
| `homebrew_basic_services_continue_until_exit` | runtime continuation through services before `svc #0` |
| `svc_halts_and_advances_pc` | default interpreter SVC halt behavior remains unchanged |

## Verification

```
cargo test -p nx86-hle -p nx86-input -p nx86-runtime --lib
just verify
```
