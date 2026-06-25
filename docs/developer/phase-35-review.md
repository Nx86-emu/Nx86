# Phase 35 Review: Barrier Semantics v0

Date: 2026-06-25
Reviewer: Codex

## What landed

| Crate | What |
|-------|------|
| `nx86-arm64-decode/src/lib.rs` | DMB/DSB/ISB decode, `InstructionClass::Barrier`, raw 4-bit option preservation, alias rendering, decoder tests |
| `nx86-ir/src/lib.rs` | `BarrierKind`, `Op::Barrier`, side-effect/result metadata, dump formatting, serde test |
| `nx86-arm64-lift/src/lib.rs` | Barrier lifting into side-effecting NxIR ops with guest boundary preservation |
| `nx86-runtime/src/lib.rs` | TinyInterpreter barrier execution as PC-advancing no-op side effects |
| `nx86-runtime/src/eval.rs` | NxIR evaluator barrier execution as no-op side effects |
| `nx86-x64-v4/src/lib.rs` | Native lowering reports barriers unsupported for Phase 35 |
| `tests/synthetic/barriers.toml` | Checked-in synthetic DMB/DSB/ISB fixture |

## Findings

### FINDING-1: native backend intentionally unsupported for barriers (info)

Phase 35 preserves ordering intent in decode/NxIR/runtime, but does not emit
x86 fences. Native lowering returns `UnsupportedOp { op: "barrier" }`; runtime
synthetic tests assert that this is the expected Phase 35 status.

### FINDING-2: barrier options are lossless (info)

Known option/domain values render with aliases, while reserved/unknown values
decode and dump as raw options. This keeps future threading, MMIO, SMC, debug,
and replay work from needing an IR schema change.

## Test coverage

| Test | What |
|------|------|
| `decodes_barriers_and_preserves_options` | DMB/DSB/ISB decode, class, aliases, raw option retention |
| `barrier_op_is_side_effecting_resultless_and_serializable` | IR metadata, operand constraints, dump text, serde round trip |
| `lifts_barriers_as_side_effecting_ir_ops` | Decode-to-NxIR barrier lifting and guest address preservation |
| `barriers_execute_as_noop_side_effects_in_interpreter_and_nxir` | Interpreter/evaluator agreement, final state, native unsupported status |
| `tests/synthetic/barriers.toml` | Public synthetic fixture for Phase 35 exit criteria |

## Verification

```
cargo test -p nx86-arm64-decode -p nx86-ir -p nx86-arm64-lift -p nx86-runtime --lib -> PASS
just verify -> PASS
```

`just verify` covered fmt, diff whitespace, clippy, workspace tests, build,
compiler/runtime smoke workers, docs with `-D warnings`, cargo audit, workflow
lint, shell lint, and the Linux x86_64 backend check. Cargo audit reported one
allowed `memmap2` warning already accepted by the repo gate.
