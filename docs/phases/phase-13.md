# Phase 13: NxIR Verifier

Phase 13 adds the `nx86-ir::verify` module. The verifier checks that a function
is well-formed before it is evaluated or lowered, and is intended to run after
lifting and after every optimization pass in debug/research builds.

Checks:

- SSA correctness: each value is defined exactly once and used only after it is
  defined; result values are within `value_count`.
- Type correctness: each operand's actual type matches the type the operation
  requires.
- Result/side-effect consistency: value-producing ops have a result slot; pure
  side-effecting ops do not.
- Legal terminators: branch targets reference existing blocks.
- Instruction boundaries: every instruction carries the guest address it came
  from (a non-empty entry block exists).

## Exit Criteria

- The verifier accepts well-formed IR and rejects invalid IR (duplicate
  definitions, use-before-def, type mismatch, missing/unexpected results,
  out-of-range branch targets).
- The verifier is available to run after lifting (wired into the lifter in
  Phase 14).
