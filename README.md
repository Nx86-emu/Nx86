# Nx86

Nx86 is a GUI-first Nintendo Switch emulator and AArch64-to-x86_64-v4 native
binary translation system.

The project is built around Continuous Dynamic Compilation: compile before play,
profile runtime fallbacks, promote discoveries, and rebuild toward higher Native
Coverage. The first product target is Linux on desktop x86_64-v4 hardware with a
Vulkan graphics backend.

## Current Prototype Target

Phase 0 through Phase 10 define the first working prototype:

- Linux x86_64-v4 target
- Rust monorepo
- egui desktop shell
- first-launch wizard
- Library, Compile, Tests, and Settings screens
- persistent app settings
- SQLite title database foundation with placeholder title entries
- JSON-line worker IPC smoke path
- synthetic ARM64 test file loading and display
- guest CPU state and expected register comparison
- narrow AArch64 decoder for MOV/ADD/SUB/B/SVC
- tiny interpreter for synthetic integer programs
- 64 GiB VMM skeleton with software page mapping and debug dumps
- internal Vulkan boundary crate prepared for future `ash` work

The prototype does not import or run Switch software yet. The first real
software target after the early foundations is homebrew, as described in
`SPEC.md`.

## Build

```sh
cargo build --workspace
```

## Run

```sh
cargo run -p nx86-app
```

The GUI shell is intended to be run on Linux. Other platforms are not supported
targets for this phase.

## Worker Smoke

```sh
cargo run -p nx86-app -- --worker compiler-smoke
cargo run -p nx86-app -- --worker runtime-smoke
```

Worker modes emit versioned JSON-line IPC events and are used by the Compile
screen smoke UI.

## Checks

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
```

## Legal Boundary

Nx86 must not ship or request copyrighted game dumps, proprietary firmware,
console keys, proprietary SDK code, shared saves, copyrighted binary blobs, or
personal user data. Local user-provided content remains the user's
responsibility.

## Specification

`SPEC.md` is authoritative for project direction, terminology, roadmap, platform
strategy, and legal boundaries.
