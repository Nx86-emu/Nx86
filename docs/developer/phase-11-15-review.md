# Phase 11-15 Review

Date: 2026-06-20

## Scope

This review covers Phases 11 through 15 from `SPEC.md`: the synthetic drawing
demo, the NxIR core, the NxIR verifier, the AArch64 integer lifter with a
differential NxIR evaluator, and the lazy NZCV flag model. These phases are the
pure-logic front half of the binary-translation pipeline; they contain no
x86_64 code generation and run natively on the development host.

## Findings

- Phase 11 is in place: a 32-bit `STR` was added to the decoder, the tiny
  interpreter now executes against an `nx86-vmm` `GuestMemory`, synthetic tests
  may declare a `[framebuffer]` region, and the GUI Tests screen renders the
  framebuffer. `tests/synthetic/draw.toml` draws an opaque-blue 2x2 image.
- Phase 12 is in place: `nx86-ir` defines module/function/block, typed SSA
  values, integer/branch/memory operations, explicit register/flag side effects,
  guest instruction boundaries, serde round-tripping, and a text dump.
- Phase 13 is in place: `nx86-ir::verify` checks SSA correctness, operand types,
  result/side-effect consistency, legal terminators, and entry-block presence,
  and is run after lifting and after the dead-flag pass.
- Phase 14 is in place: `nx86-arm64-lift` lifts MOV, ADD/SUB immediate, logical
  register ops, branches, 32/64-bit loads/stores, and the SVC exit into verified
  NxIR with basic-block CFG construction. `nx86-runtime::evaluate` is a reference
  NxIR interpreter; `run_synthetic_test` confirms the interpreter and the NxIR
  evaluator agree on final state and observable memory.
- Phase 15 is in place: flag-setting `ADDS`/`SUBS` (and `CMP`/`CMN`) and `B.cond`
  decode and lift; NxIR records flags lazily (`SetFlags`) and materializes them
  at conditional branches and at function exit; `nx86-ir-opt` eliminates
  overwritten flags. Flag and condition semantics live once in
  `nx86-core::guest`.

## Boundary Checks

- No real game dump, firmware, key, save, or copyrighted binary import path was
  added.
- No x86_64 assembler, register allocator, JIT, AOT object format, graphics, or
  Switch HLE behavior was added; those remain Phase 16+.
- The only workspace `unsafe` code is still the Linux arena reservation block in
  `nx86-vmm` (`rg -n "unsafe " crates --glob '!target/**'` reports only that
  file).
- The NxIR evaluator is host-independent: it interprets IR rather than emitting
  machine code, so the Phase 14 differential oracle runs on the Apple Silicon
  development host.

## Verification

Passed on 2026-06-20:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets   # 83 tests, 0 failures
cargo build --workspace
```

New test coverage includes: decoder forms (loads/stores, logical register ops,
ADDS/SUBS/CMP, B.cond); the NxIR verifier accept/reject cases; the lift → verify
→ evaluate differential for integer, memory, branch, framebuffer, and
conditional-branch programs; and dead-flag elimination with preserved agreement.

## Host Note

The development host is `aarch64-apple-darwin`. Everything in Phases 11-15 is
host-independent. The Linux-only GUI smoke (`cargo run -p nx86-app`, Tests →
load `tests/synthetic/draw.toml`) should still be exercised on the Linux target.
Phases 16+ add x86_64 code generation, which must be
`cfg(target_arch = "x86_64")`-gated and verified in CI.
