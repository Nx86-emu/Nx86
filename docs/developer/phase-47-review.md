# Phase 47 Review: Guest IPC v0 and Audio Runtime Skeleton

Date: 2026-06-26
Reviewer: Codex

## What landed

| Crate | What |
|-------|------|
| `nx86-service/src/lib.rs` | binary guest IPC command/response buffers, sessions, domains, descriptors, handles, result codes |
| `nx86-audio/src/lib.rs` | `cpal` host backend, null sink fallback, stereo `f32` buffers, queue/timing counters |
| `nx86-hle/src/lib.rs` | `audout:u` service classification and runtime-supplied audio service results |
| `nx86-runtime/src/lib.rs` | audio service open/dispatch path, guest IPC parsing from guest memory, PCM enqueue test |
| `nx86-gui/src` | Settings audio status and mute toggle |

## Findings

### FINDING-1: audio needed guest memory access from SVC dispatch (fixed)

The interpreter SVC callback now receives `GuestMemory`, allowing runtime service
handlers to parse command buffers and read described guest buffers without
placing memory ownership inside HLE.

### FINDING-2: guest IPC needed a separate boundary from host IPC (fixed)

`nx86-service` owns guest service IPC concepts. `nx86-core::ipc` remains the
host GUI/worker JSON-line protocol, avoiding a misleading shared type system for
two different IPC domains.

### FINDING-3: host audio must not break headless automation (fixed)

`AudioRuntime::new` attempts `cpal` output and falls back to an explicit null
sink if the host backend or output device is unavailable. Tests use the null
sink and deterministic frame advancement.

### FINDING-4: stereo queue accounting depended on host channel count (fixed)

Audio buffers are now queued and counted as stereo PCM frames regardless of the
host output channel count. The output callback adapts each stereo frame to mono,
stereo, or wider host outputs without changing queue accounting.

### FINDING-5: dependency audit still allowed a warning (fixed)

`memmap2` was updated to 0.9.11 and the reusable `audit` recipe now runs
`cargo audit --deny warnings`, so `just verify` no longer accepts advisory
warnings.

## Test coverage

| Test | What |
|------|------|
| `command_buffer_round_trips_all_descriptor_and_handle_classes` | guest IPC binary command coverage |
| `response_buffer_round_trips_result_payload_and_objects` | guest IPC binary response coverage |
| `session_table_routes_sessions_domains_and_objects` | session/domain/object routing |
| `null_sink_tracks_queue_consumption_and_underflows` | deterministic audio timing |
| `host_output_channel_count_does_not_change_stereo_queue_accounting` | host channel adaptation |
| `homebrew_audio_service_accepts_guest_ipc_command_buffer` | runtime audio service open/dispatch from guest memory |
| `navigation_state_changes` and GUI settings tests | existing GUI state still builds with audio runtime ownership |

## Verification

```
cargo test -p nx86-service -p nx86-audio -p nx86-hle -p nx86-runtime -p nx86-gui --lib --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo audit --deny warnings
actionlint .github/workflows/ci.yml .github/workflows/linux-x86_64-v4.yml
just verify
```
