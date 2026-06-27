# Phase 46 Review: Input Runtime

Date: 2026-06-26
Reviewer: Codex

## What landed

| Crate | What |
|-------|------|
| `nx86-input/src/lib.rs` | controller button bitset, input snapshots, `gilrs` gamepad polling |
| `nx86-core/src/config.rs` | persisted input config with keyboard bindings and serde defaults |
| `nx86-runtime/src/lib.rs` | fixed-snapshot and provider-based input boot helpers plus HLE request wiring |
| `nx86-gui/src` | Settings input status, keyboard binding UI, egui keyboard capture |

## Findings

### FINDING-1: input service always returned neutral state (fixed)

`svc #4` now returns the packed input state provided in the HLE request. The
compatibility `boot_homebrew` helper still uses neutral input, while fixed and
provider-based boot helpers prove homebrew can receive non-neutral controller
state and sample repeated input service calls.

### FINDING-2: real gamepad support needed Linux system dependencies (fixed)

Adding `gilrs` pulls in Linux `libudev` support. Both GitHub workflows now
install `libudev-dev` and `pkg-config` before locked Rust builds.

The Linux x86_64-v4 static-CRT artifact builds `nx86-app` with
`--no-default-features`, which disables the host gamepad backend and uses the
deterministic unavailable-gamepad fallback instead of requiring static
`libudev`.

### FINDING-3: config compatibility needed defaults (fixed)

The new input config is covered by serde defaults, so existing config files that
pre-date Phase 46 load with gamepad polling enabled and deterministic keyboard
bindings.

## Test coverage

| Test | What |
|------|------|
| `controller_buttons_pack_stable_bits` | button bit packing and clearing |
| `gilrs_buttons_map_to_controller_actions` | host gamepad button mapping |
| `old_config_without_input_section_uses_defaults` | config compatibility |
| `homebrew_input_service_returns_injected_controller_state` | runtime `svc #4` payload |
| `homebrew_input_provider_is_sampled_for_each_input_service` | repeated `svc #4` sampling |
| `input_bindings_map_to_runtime_actions_and_keys` | GUI keyboard binding bridge |

## Verification

```
cargo test -p nx86-input -p nx86-core -p nx86-hle -p nx86-runtime -p nx86-gui --lib --locked
just verify
```
