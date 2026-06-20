# Phase 12: NxIR Core

Phase 12 fills `nx86-ir` with the NxIR data model: `Module` → `Function` →
`Block` → `Inst`. Computed values are SSA temporaries (`Value`); guest register
and memory state is modelled with explicit side-effecting operations
(`SetReg`, `Store`) so the v0 IR is verifiable without cross-block phi nodes.

The op set covers integer constants, register reads/writes, integer binary ops
(add/sub/and/or/xor), truncate/zero-extend, and memory load/store. Control flow
is expressed by block terminators (`Branch`, `Halt`, `Return`). Every
instruction records the guest instruction boundary it was lifted from.

The IR serializes with serde and renders to human-readable text via `dump()`.

## Exit Criteria

- The IR models module/function/block, typed SSA values, integer/branch/memory
  operations, and preserves guest instruction boundaries.
- NxIR round-trips through serde and renders a readable text dump.
- (Lifting decoded AArch64 into NxIR is exercised in Phase 14.)
