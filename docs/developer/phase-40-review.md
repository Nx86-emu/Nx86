# Phase 40 Review: SIMD/NEON v0

Date: 2026-06-25
Reviewer: Codex

## What landed

| Crate | What |
|-------|------|
| `nx86-arm64-decode/src/lib.rs` | basic `add v.2d` and `fadd v.2d` decode, class, disassembly |
| `nx86-ir/src/lib.rs` | side-effecting `VectorBinary` IR op and vector enums |
| `nx86-arm64-lift/src/lib.rs` | NEON lifting into `VectorBinary` |
| `nx86-runtime/src/lib.rs` | interpreter vector integer/FP lane execution |
| `nx86-runtime/src/eval.rs` | NxIR evaluator vector integer/FP lane execution |
| `nx86-x64-asm/src/lib.rs` | scalar double SSE memory op emission helpers |
| `nx86-x64-v4/src/lib.rs` | vector lane native state ABI and native lowering for integer/FP v.2d adds |
| `tests/synthetic/neon-basic.toml` | checked-in basic NEON synthetic fixture |

## Findings

### FINDING-1: native vector lowering is lane-oriented (info)

Phase 40 lowers v.2d operations lane by lane: integer adds through GPRs and FP
adds through scalar SSE double instructions. Advanced x86_64-v4 packing and
register allocation remain Phase 41 work.

### FINDING-2: synthetic fixture initializes only low lanes (info)

Current AArch64 fixture coverage uses scalar `fmov` to seed low lanes, so unit
tests also cover state layout and native lane lowering. Broader NEON immediate
or load coverage will let future fixtures exercise high-lane nonzero data.

### FINDING-3: invalid native vector lanes are rejected (fixed)

Audit found that native vector state addressing treated any non-zero lane as
lane 1. The helper now rejects lanes outside `0..2` instead of aliasing them.

## Test coverage

| Test | What |
|------|------|
| `decodes_basic_neon_ops` | NEON decode and disassembly |
| `lifts_scalar_fp_and_neon_ops` | NEON lift into side-effecting NxIR |
| `neon_synthetic_test_passes` | interpreter/NxIR exactness and native availability/status gating |
| `emits_scalar_double_sse_memory_ops` | assembler SSE emission |
| `native_memory_abi_offsets_are_stable` | expanded native state ABI offsets |
| `vector_lane_addressing_rejects_invalid_lane` | native vector state addressing rejects invalid lane indexes |
| `lowers_scalar_fp_and_vector_ops` | x86_64-v4 lowerer accepts vector ops |

## Verification

```
cargo test -p nx86-arm64-decode -p nx86-ir -p nx86-arm64-lift -p nx86-runtime -p nx86-x64-asm -p nx86-x64-v4 --lib -> PASS
```
