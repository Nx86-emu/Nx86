# Phase 28 Review

Date: 2026-06-22

## Scope

This review covers Phase 28 from `SPEC.md`: define a guard IR construct, define a
deopt point, store deopt metadata, handle guard failure, and crash on deopt
failure. The exit criterion is that a failed guard routes to the deopt handler.
Authoritative semantics are SPEC §20 (Deoptimization).

## Findings

- A guard is modelled as `Terminator::Guard { cond, if_pass, deopt }` in
  `nx86-ir/src/lib.rs`. NxIR is one-terminator-per-block SSA, and a failed guard
  transfers control, so a terminator (not an `Op`) is the correct shape. It reuses
  the lazy-NZCV machinery `CondBranch` already relies on. `successors()` returns
  only `if_pass`; the deopt edge is a documented side-exit, not a block successor.
- Deopt points live in `DeoptPoint { resume_pc, reason }` within
  `Function.deopt_points`, referenced by `DeoptId`. The field is `#[serde(default)]`
  so older serialized IR still deserializes. `Function::dump` renders both the
  guard terminator and the deopt table, so the Phase 27 Inspector shows them with
  no extra work.
- The verifier (`verify.rs`) range-checks each guard's `DeoptId`
  (`DeoptPointOutOfRange`); the `if_pass` target is covered by the existing
  successor/`BranchTargetOutOfRange` check. This enforces "no guard without
  recovery" — a verified guard always has a real deopt point.
- The evaluator (`nx86-runtime/src/eval.rs`) now returns `EvalOutcome`: `Exit` for
  a normal halt/return, `Deopt { state, deopt, reason }` for a failed guard (with
  `pc` set to the resume PC). Per the codex design review, deopt is reported as an
  evaluator outcome rather than a `CpuState` flag, because it is a
  translator-internal recovery event, not guest-visible state; this also keeps the
  differential oracle's full-state equality clean. A guard with no recovery point
  yields `EvalError::DeoptFailure` — a loud, non-continuing failure (SPEC §20.4).
- Native guard emission is deferred: the `nx86-x64-v4` lowerer reports
  `UnsupportedTerminator { "guard" }`, exactly as it does for `CondBranch`, because
  native flag materialization does not exist yet. The three halt-reason extractors
  in `nx86-backend` and `nx86-jit` treat a guard as not-a-halt.

## Boundary Checks

- Pure logic: no native code is executed, no new `unsafe`, and no new
  mmap/transmute boundaries. Guards are evaluated host-independently and lowering
  reports them unsupported.
- No game dumps, firmware, keys, or copyrighted blobs are introduced; guards and
  deopt points are synthetic IR constructs built by hand in tests (lifted code
  never emits guards).
- No `unwrap`/`todo`/`dbg` in the new code. Deopt failure surfaces as a typed
  `EvalError`, never a panic, and the verifier prevents it for verified IR.

## Verification

Passed locally on `aarch64-apple-darwin`:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
```

New tests: three in `nx86-ir` verify (a valid guard; an out-of-range deopt id; an
out-of-range pass target), one in `nx86-ir` for guard/deopt-table dump rendering
plus a serde round-trip, three in `nx86-runtime` eval (a holding guard continues;
a failed guard routes to its deopt point; a missing deopt point is a deopt
failure), and one in `nx86-x64-v4` (lowering reports guards unsupported). All are
host-independent and run on the dev host.

## Note on tooling

Per the user's request, the codex CLI (via the Duo skill) assisted this phase: a
read-only design second-opinion that endorsed guard-as-terminator and the deopt
table, and recommended reporting deopt via an evaluator outcome rather than a
`CpuState` flag (adopted); and a workspace-write pass that applied the mechanical
`Terminator::Guard` match arms and `deopt_points` literal updates across the five
non-`nx86-ir` crates. The core IR/verifier/evaluator logic and all verification
were done directly.
