# Phase 28: Guard and Deopt Metadata v0

Phase 28 adds the first speculation-safety primitive to NxIR: a **guard** that
tests an assumption and, when it fails, side-exits to a **deopt point** that
reconstructs guest-visible state and resumes. This is the SPEC §20 rule made
concrete — *"No speculation without deopt. No guard without recovery."*

This is a host-independent v0 living in the NxIR data model and the NxIR
evaluator (the project's differential oracle). Native execution is Linux-only and
unavailable on the dev host, and the native lowerer does not yet materialize NZCV
flags or lower conditional branches, so *native* guard emission is deferred (the
lowerer reports guards as unsupported, exactly as it already does for
`CondBranch`). The phase is about the IR + metadata + recovery routing, which is
fully testable on every host.

## What it does

- **Guard terminator** — `Terminator::Guard { cond, if_pass, deopt }`. A guard is
  a terminator because a failed guard transfers control, and NxIR blocks carry
  exactly one terminator. It materializes NZCV from the current lazy flag source
  (the same machinery as `CondBranch`) and tests `cond`. If it holds, control
  continues to the `if_pass` block; otherwise the guard fails and control
  side-exits to the deopt point. `successors()` returns only `if_pass` — the
  deopt edge leaves the function, like `Halt`.
- **Deopt point + metadata table** — `DeoptPoint { resume_pc, reason }` stored in
  `Function.deopt_points: Vec<DeoptPoint>` and referenced by `DeoptId`. v0 records
  the guest PC to resume at plus a reason; the live evaluator already holds the
  rest of the guest-visible state. The table renders in `Function::dump`, so the
  Phase 27 Inspector NxIR view surfaces guards and deopt points unchanged.
- **Guard failure handling (the deopt handler)** — the evaluator routes a failed
  guard to its deopt point: it sets `pc = resume_pc` and returns
  `EvalOutcome::Deopt { state, deopt, reason }`. Deopt is a translator-internal
  recovery exit, **not** guest-visible architectural state, so it is reported via
  the evaluator's `EvalOutcome` rather than as a `CpuState` flag (unlike `halt`,
  which models a real guest `SVC`).
- **Crash on deopt failure** — a guard referencing a non-existent deopt point is a
  deopt failure: the verifier rejects it at verify time
  (`VerifyError::DeoptPointOutOfRange`), and the evaluator defends with a hard
  `EvalError::DeoptFailure` that stops evaluation rather than continuing with
  unrecovered state (SPEC §20.4). A verified function can never deopt-fail.

## New types

- `nx86-ir`: `DeoptId(u32)`, `DeoptPoint { resume_pc, reason }`,
  `Function.deopt_points`, `Terminator::Guard`,
  `VerifyError::DeoptPointOutOfRange`.
- `nx86-runtime`: `EvalOutcome { Exit(CpuState), Deopt { state, deopt, reason } }`
  (now returned by `evaluate`), `EvalError::DeoptFailure`.

## Exit Criteria

- A failed guard routes to the deopt handler: the evaluator test
  `failed_guard_routes_to_deopt_handler` runs a guard whose condition does not
  hold and asserts the result is `EvalOutcome::Deopt` with the deopt point's
  `resume_pc` and reason; a holding guard continues to its pass block; a guard
  with no recovery point yields `EvalError::DeoptFailure`.
