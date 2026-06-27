# Phase 46: Input Runtime

Phase 46 replaces the neutral Phase 45 input stub with a runtime input path.
Homebrew can now receive a packed controller state through `svc #4`, sourced
from keyboard mapping, gamepad polling, or an injected test snapshot.

## What it does

- **Controller state** - `nx86-input` defines stable button bits, controller
  snapshots, and action labels for homebrew-visible input.
- **Gamepad backend** - `nx86-input` polls connected controllers through
  `gilrs` 0.11.2 and maps common gamepad buttons to the Nx86 controller state.
  This backend is a default Cargo feature; release artifacts may disable it and
  fall back to the deterministic unavailable-gamepad state.
- **Keyboard mapping** - `nx86-core` persists default keyboard bindings in
  `AppConfig`, and `nx86-gui` translates egui key state into controller bits.
- **Controller config UI** - Settings shows gamepad backend status, current
  packed controller state, and editable keyboard bindings.
- **Runtime input service** - `boot_homebrew_with_input` passes a fixed input
  snapshot into HLE, while `boot_homebrew_with_input_provider` samples input for
  each `svc #4`; both return `x0 = 0` and `x1 = controller_state`. The existing
  `boot_homebrew` helper still uses neutral input for compatibility.

## Design

The input runtime keeps host dependencies out of the HLE layer. HLE receives a
plain packed input value in `SvcRequest`; the runtime owns the snapshot/provider
used for each boot, and the GUI owns keyboard capture from egui. This keeps the
service ABI deterministic while still allowing host gamepads through `gilrs`.

`AppConfig` uses serde defaults for the new input section, so existing config
files without input settings continue to load and receive the default keyboard
mapping.

## Phase boundary

Phase 46 implements button input only. It does not cover controller rumble,
motion sensors, per-title controller profiles, hotplug notifications beyond
backend status, or real Horizon input ABI fidelity. Those remain later runtime
or compatibility work.
