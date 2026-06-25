# Phase 35: Barrier Semantics v0

Phase 35 adds lossless AArch64 barrier representation for DMB, DSB, and ISB.
Barriers now decode, lift into NxIR, execute through the interpreter and NxIR
evaluator, and remain visible as side-effecting ordering markers.

## What it does

- **Decoder** - `BarrierKind::{Dmb,Dsb,Isb}` and
  `InstructionKind::Barrier { kind, option }`. The decoder accepts every 4-bit
  option value and renders known aliases such as `sy`, `ld`, `ishst`, and
  `nsh`; reserved/unknown values remain preserved as raw `#0xN` options.
- **NxIR** - `Op::Barrier { kind, option }` plus `BarrierKind`. Barrier ops are
  side-effecting, resultless, operandless, serializable, and visible in text
  dumps as `barrier.<kind> <option>`.
- **Lifter** - DMB/DSB/ISB lift to NxIR barrier ops at their original guest
  instruction boundary.
- **TinyInterpreter** - barriers advance PC and do not mutate CPU or memory
  state in v0.
- **Evaluator** - NxIR barrier ops execute as no-op side effects so interpreter
  and evaluator state stay aligned.
- **Synthetic fixture** - `tests/synthetic/barriers.toml` exercises DMB, DSB,
  and ISB before a simple integer add.

## Design

Phase 35 is intentionally semantic-preservation first. A barrier is not lowered
to a host fence yet, but it is also not deleted: it is an explicit side effect in
the IR, so future optimizer, threading, MMIO, SMC, debug, and replay work can
reason about the original AArch64 ordering intent.

The raw option/domain operand is preserved even when Nx86 can render a known
alias. This avoids a later schema break when Phase 36+ starts assigning
thread-visible meaning to barrier domains.

## Phase boundary

Phase 35 owns DMB/DSB/ISB decode, disassembly, NxIR representation, lifting,
interpreter/evaluator execution, and tests. It does not own native x86 fence
lowering, scheduler/thread semantics, MMIO/service ordering, code-cache
invalidation semantics, or replay-log barrier events.
