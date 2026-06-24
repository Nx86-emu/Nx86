# Phase 31 Review

Date: 2026-06-23

## Scope

This review covers Phase 31 from `SPEC.md`: slowmem counters, profile events,
fault reporting, and the Native Coverage penalty. The authoritative semantics
are SPEC §16.2, §16.3, §18.2, and §25.4.

## Findings

- `SlowmemCounters` in `nx86-backend/src/lib.rs` tracks total slowmem callback
  invocations, calls grouped by reason code (`BTreeMap<String, u64>`), and calls
  grouped by guest PC (`BTreeMap<u64, u64>`). `top_sources(n)` returns the
  hottest guest PCs sorted by count descending.
- `FaultReport` captures faulting address, access kind, guest PC, source block
  PC, page permissions, and fault kind. `Display` formats a human-readable crash
  report. The remaining SPEC 25.4 fields (guest thread, call stack, native
  object, page generation, recent executable writes, scheduler context) are
  deferred to future phases.
- `GuestMemory::page_permissions(address)` returns `Option<PagePermissions>` for
  mapped pages or `None` for unmapped pages. Two unit tests verify both paths.
- The `slowmem_read` and `slowmem_write` callbacks now: (1) derive a reason code
  from the fastmem state (`no_fastmem`, `no_permissions`, `checked`), (2) record
  it in `SlowmemCounters`, (3) push a `ProfileEvent::Slowmem` into
  `pending_events`. Fault paths use the VMM fault kind as the reason code.
- `NativeMemoryContext` carries `counters: SlowmemCounters` and
  `pending_events: Vec<ProfileEvent>`. New methods: `take_pending_events()`,
  `counters()`, `build_fault_report()`.
- `DispatchError::Memory` is now a struct variant with `error` and `report`
  fields. The dispatch loop constructs a `FaultReport` from the error and the
  current source block PC before returning.
- The dispatch loop drains `memory_context.take_pending_events()` after each
  block, emitting them to the profile sink. This batches per-call events at
  block granularity.
- `DispatchOutcome` carries `slowmem_counters: SlowmemCounters` from the
  `NativeMemoryContext`. All four return sites (Halt, MissingBlock x2, StepLimit)
  include the counters.
- `NativeOutcome` gains `slowmem_counters: Option<SlowmemCounters>`.
  `run_dispatched_function_with_memory` propagates counters from the dispatch
  outcome. The tiny-block path includes counters from its `NativeMemoryContext`.
- `CoverageSnapshot` gains `slowmem_calls: u64` and `total_accesses: u64`.
  `rebuild_from_profile` counts `ProfileEvent::Slowmem` entries in the profile
  and passes the count into the snapshot.

## Boundary and safety checks

- No new `unsafe` code was introduced. The existing `unsafe` in the callbacks
  is unchanged; `build_fault_report` accesses `GuestMemory` through the same
  `NonNull` pointer lifetime as the callbacks.
- The `#[allow(unsafe_code)]` annotation on `build_fault_report` covers the
  `page_permissions` query through the borrowed `NonNull<GuestMemory>`.
- `FaultReport` does not expose raw memory contents, game data, or personal
  information. It contains only addresses, access metadata, and page flags.
- No `unwrap`, `todo`, `dbg`, or new `unsafe` was introduced.
- No proprietary data, firmware, keys, game content, or personal data was
  introduced.

## Verification

Passed locally on `aarch64-apple-darwin`:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
```

All 206 tests pass. The new `page_permissions` tests in `nx86-vmm` verify both
mapped and unmapped paths. The existing dispatch and synthetic tests exercise
the slowmem callback paths through the differential testing oracle.
