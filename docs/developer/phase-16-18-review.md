# Phase 16-18 Review

Date: 2026-06-21

## Scope

This review covers Phases 16 through 18 from `SPEC.md`: the internal x86_64
assembler skeleton, executable-memory manager, and first native NxIR block. The
implemented path is intentionally narrow: a verified single-block integer NxIR
function can lower to x86_64 bytes and, on Linux x86_64, run as trusted
generated code.

## Findings

- Phase 16 is in place: `nx86-x64-asm` exposes `Assembler`, `CodeBuffer`,
  `Label`, `Reg64`, `Mem64`, and `AsmError`, with labels, stack-frame helpers,
  register/memory operands, and exact byte tests for the supported encodings.
- Phase 17 is in place: `nx86-jit` owns W^X executable memory on Linux x86_64,
  rejects empty code, reports unsupported hosts cleanly, and exposes execution as
  an explicit unsafe API contract backed by narrow call wrappers.
- Phase 18 is in place: `nx86-x64-v4` lowers the tiny single-block NxIR subset
  (`Const`, `GetReg`, `SetReg`, `Binary Add/Sub i64`, `Halt`/`Return`) into
  x86_64 bytes using `NativeBlockState`; unsupported IR returns structured
  errors.
- `nx86-backend` orchestrates native execution and comparison, while
  `nx86-runtime` lifts/optimizes/verifies once before running the NxIR evaluator
  and native attempt from the same function.
- The GUI Tests screen now shows a compact native x86_64 status and assembler
  dump alongside the existing NxIR dump.

## Boundary Checks

- No title import, firmware, keys, commercial software path, HLE behavior,
  register allocator, object cache, dispatcher, memory lowering, flags lowering,
  or branch lowering was added.
- Native code execution is host-gated: Apple Silicon reports unavailable; Linux
  x86_64 owns the callable-code tests.
- mmap/transmute `unsafe` is limited to `nx86-jit`'s Linux executable-memory
  platform module; `nx86-backend` has a single documented unsafe call-site
  acknowledgement for the bytes emitted by `lower_tiny_block`. Existing VMM
  unsafe remains unchanged.

## Verification

Passed locally on `aarch64-apple-darwin`:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets   # 97 tests, 0 failures
cargo build --workspace
```

The Linux x86_64-only tests for calling generated code are compiled behind
`#[cfg(all(target_os = "linux", target_arch = "x86_64"))]` and were not executed
on the Apple Silicon development host.
