# Phase 15: Lazy Flags Model

Phase 15 adds NZCV condition flags with a lazy model. The decoder gains
`ADDS`/`SUBS` immediate (and the `CMP`/`CMN` aliases) and `B.cond`. The shared
flag semantics live in `nx86-core::guest`: `Nzcv::from_add`/`from_sub` compute
NZCV, `Cond` plus `Nzcv::satisfies` evaluate AArch64 condition codes — one
source of truth used by both execution engines.

NxIR represents flags lazily: `Op::SetFlags { op, lhs, rhs }` records the flag
*source* without computing NZCV, and `Terminator::CondBranch` materializes NZCV
from that source only when a branch reads it. The NxIR evaluator also
materializes at function exit so the architectural NZCV stays observable in the
final state.

`nx86-ir-opt::eliminate_dead_flags` removes any `SetFlags` that is overwritten
by a later `SetFlags` in the same block (flags are only consumed by a block
terminator). The differential harness runs this pass and re-verifies the IR
before evaluating, per `SPEC.md` §21.5.

## Exit Criteria

- Conditional branches work through lazy flags: the AArch64 interpreter (eager
  NZCV) and the NxIR evaluator (lazy materialization) agree on taken and
  not-taken branches.
- Tests cover overwritten flags: two `CMP`s in a block lift to two `SetFlags`,
  the dead-flag pass leaves one, and the engines still agree.
