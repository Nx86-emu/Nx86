# Phase 31: Slowmem v0

Phase 31 adds slowmem observability on top of the Phase 30 fast/slow memory
split. The native code generator already emits fastmem checks and slowmem
callbacks; this phase wires counters, profile events, a crash report struct, and
the Native Coverage penalty fields.

## What it does

- **Slowmem counters** — `SlowmemCounters` tracks total slowmem callback
  invocations, calls grouped by reason code, and calls grouped by guest PC.
  The `slowmem_read` and `slowmem_write` callbacks increment counters on every
  slow-path invocation. `top_sources(n)` returns the hottest guest PCs.
- **Profile events** — every slowmem callback (success or failure) pushes a
  `ProfileEvent::Slowmem` into a per-dispatch buffer. The dispatcher drains
  pending events after each block, batch-emitting them to the profile sink.
  This avoids per-access overhead in the native hot path.
- **Fault reports** — `FaultReport` captures faulting address, access kind,
  guest PC, source block PC, page permissions, and fault kind. The dispatcher
  returns `DispatchError::Memory { error, report }` instead of a bare error,
  so callers can display structured crash information.
- **Page permission query** — `GuestMemory::page_permissions(address)` returns
  `Some(PagePermissions)` for mapped pages or `None` for unmapped pages, giving
  fault reports access to the page's current state.
- **Coverage snapshot** — `CoverageSnapshot` gains `slowmem_calls` and
  `total_accesses` fields. `rebuild_from_profile` counts `ProfileEvent::Slowmem`
  entries in the profile and passes the count into the snapshot.
- **Dispatch outcome** — `DispatchOutcome` carries `slowmem_counters` so callers
  can inspect aggregate slowmem behavior after a dispatch run.
- **Native outcome** — `NativeOutcome` gains `slowmem_counters:
  Option<SlowmemCounters>` for differential test verification.

## Phase boundary

Phase 31 establishes the slowmem penalty mechanism and crash report struct. It
does not own: memory mirroring (Phase 32), concurrent mapping mutation, SMC
invalidation, full Native Coverage GUI display (Phase 43), slowmem-to-fastmem
promotion counters, IPC crash events, or the extended FaultReport fields (guest
thread, call stack, native object, page generation, recent executable writes,
scheduler context).

## Exit criteria

An invalid memory access pauses dispatch, returns a structured `FaultReport`
with faulting address / access type / guest PC / page permissions, and slowmem
counters are populated and visible in `DispatchOutcome`.
