# Phase 39 Review: FP Scalar Support

Date: 2026-06-25
Reviewer: Codex

## What landed

| Crate | What |
|-------|------|
| `nx86-core/src/guest.rs` | vector lane helpers and scalar `f64` accessors over existing FP/SIMD registers |
| `nx86-arm64-decode/src/lib.rs` | scalar double `fmov`, `fadd`, `fsub`, `fmul`, `fdiv`, `fcmp` decode and tests |
| `nx86-ir/src/lib.rs` | side-effecting scalar FP IR ops and serde/dump coverage |
| `nx86-arm64-lift/src/lib.rs` | scalar FP lifting into NxIR |
| `nx86-runtime/src/lib.rs` | interpreter scalar FP execution and FP compare NZCV semantics |
| `nx86-runtime/src/eval.rs` | NxIR evaluator scalar FP execution and lazy-flag preservation fix |
| `nx86-x64-v4/src/lib.rs` | native state carries FP/SIMD lanes, FPCR, and FPSR; scalar FP arithmetic lowering |
| `tests/synthetic/scalar-fp.toml` | checked-in scalar FP synthetic fixture |

## Findings

### FINDING-1: native `fcmp` lowering is deferred (info)

Scalar FP arithmetic has native lowering support, but `FpCompare` returns
`UnsupportedOp { op: "fp compare" }`. This keeps host flag mapping and unordered
compare semantics out of Phase 39 while interpreter/NxIR exactness is covered.

### FINDING-2: FP immediate support is intentionally narrow (info)

The decoder recognizes the assembler-verified immediate encodings used by the
Phase 39 fixtures (`#1.0` and `#2.0`). Full AArch64 modified FP immediate
expansion remains future work.

## Test coverage

| Test | What |
|------|------|
| `decodes_scalar_fp_ops` | scalar FP decode, class, disassembly, exact immediate bits |
| `lifts_scalar_fp_and_neon_ops` | scalar FP lift into side-effecting NxIR |
| `fp_and_vector_ops_are_side_effecting_and_serializable` | IR metadata, dump, serde |
| `scalar_fp_synthetic_test_passes` | interpreter/NxIR exactness for fmov/add/sub/mul/div/fcmp |
| `lowering_rejects_fp_compare_for_now` | explicit native boundary for `fcmp` |

## Verification

```
cargo test -p nx86-arm64-decode -p nx86-ir -p nx86-arm64-lift -p nx86-runtime -p nx86-x64-v4 --lib -> PASS
```
