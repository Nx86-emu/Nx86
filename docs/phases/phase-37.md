# Phase 37: Scheduler Replay v0

Phase 37 adds bounded scheduler replay logs so a recent crash window can be
inspected deterministically.

## What it does

- **Replay log** - `ReplayLog` stores spawn, dispatch, yield, halt, and crash
  events with monotonically increasing sequence numbers.
- **Size cap** - logs retain only the configured number of newest events and
  count dropped entries in `ReplayMetadata`.
- **Replay metadata** - retained count, dropped count, first/last sequence,
  scheduler mode, thread count, and cap are reported together.
- **Crash window** - `Scheduler::crash_thread` records a crash event and returns
  a `CrashWindow` filtered to the affected thread.
- **Analysis** - `ReplayAnalysis` summarizes dispatches, yields, halt/crash
  state, and last observed PC for the crash window.
- **Developer UI** - the Scheduler screen surfaces replay retention metadata for
  the synthetic run.

## Design

The replay log is a small deterministic ring-style window represented as a
bounded vector. Dropping oldest events is explicit through metadata, so a crash
report can say whether the retained window is complete or truncated.

## Phase boundary

Phase 37 owns scheduler replay events, caps, metadata, and crash-window
analysis. It does not own persistent crash files, full runtime trace capture,
barrier replay semantics, or service-level replay.
