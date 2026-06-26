# Phase 44: Homebrew Loader v0

Phase 44 adds the first user-provided homebrew boot path. It loads a simple,
explicit Nx86 homebrew descriptor, maps code/data/stack memory, starts execution
at the declared entrypoint, and classifies the first minimal SVC exit.

## What it does

- **Homebrew descriptor** - `nx86-import` parses `.nxhb.toml` modules with
  format version, metadata, code load address, entrypoint, stack settings, ARM64
  bytes, and optional data segments.
- **Module metadata** - descriptors carry a display name, version, author, and
  optional module id without requiring firmware, keys, or SDK metadata.
- **Entrypoint handling** - the runtime seeds guest PC from the module entrypoint
  and SP from the module stack top before interpreting guest code.
- **Memory mapping** - the loader maps code, optional segments, and stack pages
  into `GuestMemory`; code is write-seeded and then reprotected read/execute.
- **Minimal services** - `nx86-hle` recognizes `svc #0` as a clean homebrew exit
  using x0 as the exit code. Unknown SVCs are surfaced as unhandled service
  calls.
- **Title storage** - `nx86-title-db` records a `homebrew` source kind and
  persists the module TOML under the title content directory.

## Design

Phase 44 uses an Nx86-owned `.nxhb.toml` descriptor rather than encrypted or
platform-native Switch containers. That keeps the first boot path deterministic,
legal-boundary friendly, and aligned with the current synthetic ARM64 execution
surface. The runtime boot helper still uses the interpreter path, so this phase
proves loader integration and entrypoint state before service breadth or native
container parsing arrives.

## Phase boundary

Phase 44 owns simple local homebrew descriptors, metadata, entrypoint/stack
setup, memory mapping, and `svc #0` exit classification. Phase 45 still owns
service call dispatch breadth, filesystem/thread/memory/input service skeletons,
and higher-level HLE behavior. Real NRO/NSO parsing, SDK ABI coverage, dynamic
linking, graphics/audio/input runtime behavior, and commercial-title loading
remain later phases.
