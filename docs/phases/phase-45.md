# Phase 45: HLE/Service Skeleton

Phase 45 turns the Phase 44 homebrew exit classifier into a small service
dispatcher. It keeps the ABI synthetic and `.nxhb.toml`-scoped while proving
that a homebrew program can call multiple basic services and continue execution
before exiting.

## What it does

- **Service dispatch** - `nx86-hle` accepts a typed SVC request and maps
  `svc #0` through `svc #4` to deterministic skeleton outcomes.
- **Filesystem skeleton** - `svc #1` returns success without touching host or
  guest files.
- **Thread skeleton** - `svc #2` returns success plus the current synthetic
  guest thread id.
- **Memory skeleton** - `svc #3` returns success plus the 4 KiB synthetic page
  size.
- **Input skeleton** - `svc #4` returns success plus a neutral controller state.
- **Runtime continuation** - `boot_homebrew` handles non-exit services by
  writing x0/x1 results and continuing interpretation until exit or an
  unhandled service.

## Design

The dispatcher remains dependency-free and receives only the SVC immediate,
argument registers, and synthetic thread id. The runtime owns conversion from
`CpuState` to an HLE request and applies HLE register results back into the
guest state. This keeps HLE independent of the CPU, VMM, and scheduler crates
while still giving homebrew boot a concrete service boundary.

The default `TinyInterpreter` path still halts on any `svc`, preserving
synthetic-test behavior. Only the homebrew boot path installs the service
handler.

## Phase boundary

Phase 45 does not implement Horizon IPC, real filesystem access, host thread
creation, memory mapping services, or live input devices. It only establishes
stable skeleton service dispatch, deterministic return values, and event
reporting so later phases can widen each service without changing the loader
entrypoint.
