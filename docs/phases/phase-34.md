# Phase 34: Atomics v0

Phase 34 adds guest atomic instruction support: exclusive monitor model,
LDXR/STXR decoding/execution, and LDAR/STLR acquire/release semantics.

## What it does

- **Exclusive monitor** — `ExclusiveMonitor` struct on `CpuState` tracks
  monitored guest address + region size. `set_monitor(addr, size)` on LDXR;
  `clear_monitor()` on STXR success/fail or any intervening exclusive store.
- **Decoder** — `LoadExclusive`, `StoreExclusive`, `LoadAcquire`,
  `StoreRelease` variants on `InstructionKind`. `InstructionClass::Atomic`.
  Decodes the `0xC8`/`0x88` encoding space (bits [29:27]=001).
- **NxIR** — 4 new `Op` variants: `LoadExclusive`, `StoreExclusive` (returns
  I32 status: 0=success, 1=fail), `LoadAcquire`, `StoreRelease`. All are
  side-effecting.
- **Lifter** — lifts all 4 instruction kinds. LDXR/STXR → monitor-aware
  exclusive ops. LDAR/STLR → acquire/release ops (plain read/write in v0).
- **Evaluator** — `LoadExclusive` reads memory + sets monitor. `StoreExclusive`
  checks monitor match → write + clear + status 0, or skip + clear + status 1.
  `LoadAcquire`/`StoreRelease` = plain read/write (v0 single-thread).
- **TinyInterpreter** — same semantics as evaluator for all 4 instruction kinds.
- **Native backend** — returns `UnsupportedOp` for atomic ops (deferred).

## Design

Single-thread v0 model. Monitor is per-`CpuState`. LDXR sets monitor; STXR
checks address+size match → success or failure. Acquire/release are no-op
barriers (x86 TSO is stronger than AArch64, so plain MOVs suffice). Native
lowering deferred — interpreter/evaluator provide correctness.

## Phase boundary

Phase 34 owns exclusive monitor model, LDXR/STXR/LDAR/STLR decode+lift+execute,
and atomic unit tests. It does not: CAS, atomic RMW, multi-thread monitor
clearing, native x86 LOCK prefix lowering, or barrier semantics (Phase 35).
