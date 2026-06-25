# Phase 36: Guest Threading v0

Phase 36 adds the first deterministic guest-thread model. It does not execute
real guest OS threads yet; it establishes the Nx86-owned state shape that later
runtime and service layers can drive.

## What it does

- **Scheduler crate** - `nx86-scheduler` now owns `Scheduler`,
  `GuestThread`, `SchedulerMode`, host-thread mapping metadata, and synthetic
  guest-thread runs.
- **Thread state** - each scheduled guest thread carries the existing
  `CpuState::thread()` metadata, guest PC, status, deterministic index, and CPU
  tick count.
- **Host mapping** - host-thread mode maps each guest thread to a deterministic
  host-thread index. Fiber mode pins synthetic threads to host thread 0 and
  assigns a fiber slot.
- **GUI view** - the GUI has a `Scheduler` screen that renders thread ID, name,
  state, PC, host index, fiber slot, and per-thread CPU ticks.
- **Synthetic proof** - `SyntheticThreadProgram::two_thread_counter()` runs two
  guest threads to completion and records per-thread CPU use.

## Design

The v0 model is intentionally deterministic and side-effect free outside the
scheduler crate. It gives later OS/HLE work a stable guest-thread identity and
CPU-accounting surface without committing to real host execution, locking, or
guest-kernel semantics.

## Phase boundary

Phase 36 owns guest-thread records, host mapping metadata, GUI rows, and
synthetic multi-thread execution. It does not own real host thread creation,
guest kernel scheduling, synchronization primitives beyond existing atomics and
barriers, or service/HLE thread APIs.
