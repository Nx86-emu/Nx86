# Phase 41 Review: Advanced x86_64-v4 Vector Lowering

Date: 2026-06-25
Reviewer: Codex

## What landed

| Crate | What |
|-------|------|
| `nx86-ir/src/lib.rs` | `VectorCompare`, `VectorShuffle`, compare/shuffle enums, dump and serde coverage |
| `nx86-runtime/src/eval.rs` | vector equality mask and d-lane swap evaluator behavior |
| `nx86-x64-asm/src/lib.rs` | packed SIMD emitters for `movdqu`, `paddq`, `addpd`, `pshufd`, AVX-512 `vpcmpeqq`, and `vpmovm2q` |
| `nx86-x64-v4/src/lib.rs` | packed vector lowering for add, FP add, equality masks, and shuffles |

## Findings

### FINDING-1: Phase 40 scalarized vector ops (fixed)

Phase 40 lowered v.2d adds lane by lane. Phase 41 now emits packed SIMD ops and
AVX-512 opmask compares for the current vector IR surface, reducing NEON-heavy
synthetic lowering to one packed operation plus load/store per vector op.

### FINDING-2: vector compare/shuffle had no IR form (fixed)

NxIR now has explicit side-effecting vector compare and shuffle operations, with
evaluator and native lowering coverage.

### FINDING-3: spill strategy remains instruction-local (info)

The current vector IR writes guest SIMD registers directly, so the v0 spill
boundary is `NativeBlockState` plus instruction-local XMM scratch registers.
SSA vector temporaries will need a wider vector allocator when NxIR grows them.

## Test coverage

| Test | What |
|------|------|
| `fp_and_vector_ops_are_side_effecting_and_serializable` | IR metadata, dump text, serde for vector compare/shuffle |
| `advanced_vector_compare_and_shuffle_update_simd_state` | evaluator mask and shuffle semantics |
| `emits_packed_vector_memory_ops` | assembler packed SIMD encodings/dumps |
| `emits_avx512_mask_compare_bytes` | AVX-512 opmask compare and mask materialization encodings |
| `lowers_advanced_vector_ops_with_masks_and_shuffle` | x86_64-v4 native compare/shuffle lowering |
| `lowers_scalar_fp_and_vector_ops` | packed `paddq`/`addpd` lowering for existing vector add ops |

## Verification

```
cargo test -p nx86-ir -p nx86-runtime -p nx86-x64-asm -p nx86-x64-v4 --lib -> PASS
```
