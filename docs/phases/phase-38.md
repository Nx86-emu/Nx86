# Phase 38: Fiber/Task Mode Prototype

Phase 38 adds an optional fiber/task scheduling mode for synthetic guest
threads and compares it with host-thread mode.

## What it does

- **Mode switch** - `SchedulerMode::{HostThreads,Fibers}` selects whether
  synthetic guest threads map to unique host-thread indexes or fiber slots on
  host thread 0.
- **Fiber metadata** - `HostThreadMapping` records `fiber_slot` for fiber-mode
  guest threads.
- **Synthetic support** - the same `SyntheticThreadProgram` used by host-thread
  mode runs under fiber mode.
- **Comparison** - `compare_host_threads_and_fibers` runs both modes and checks
  their deterministic replay traces for equality.
- **GUI view** - the Scheduler screen shows fiber slots and whether the sample
  host-thread/fiber traces match.

## Design

Fiber mode is a scheduler policy prototype, not a coroutine runtime. It proves
that the guest-thread model does not depend on one host thread per guest thread
and that deterministic traces can remain identical across modes.

## Phase boundary

Phase 38 owns optional synthetic fiber/task scheduling, metadata, and
host-vs-fiber comparison. It does not own stackful fibers, async I/O,
preemption, guest kernel integration, or production task stealing.
