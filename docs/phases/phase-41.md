# Phase 41: Advanced x86_64-v4 Vector Lowering

Phase 41 replaces the Phase 40 lane-by-lane vector path with packed native
lowering for the current v.2d SIMD surface.

## What it does

- **Vector register mapping** - guest v0..v31 still live in `NativeBlockState`,
  but each vector op maps its source to an XMM scratch register and writes the
  packed result back as one 128-bit value.
- **Mask register use** - `VectorCompare::EqI64` uses AVX-512 opmask register
  `k1`, then materializes the mask back into the guest SIMD register file.
- **Shuffle lowering** - `VectorShuffle::SwapD` lowers to `pshufd` with the
  lane-swap immediate for v.2d registers.
- **Compare lowering** - NxIR and the evaluator now model vector equality masks,
  and the x86_64-v4 backend emits `pcmpeqq`.
- **Spill strategy** - vector lowering uses instruction-local XMM scratch state
  and the `NativeBlockState` SIMD file as the spill/reload boundary. Existing
  scalar SSA spills remain stack-based.

## Design

The packed lowering path emits `movdqu`, `paddq`, `addpd`, `pcmpeqq`, and
`pshufd`, plus `vpcmpeqq`/`vpmovm2q` for mask compares, instead of scalarizing
each 64-bit lane through GPRs. This removes the Phase 40 per-lane lowering
bottleneck for the current vector IR surface.

## Phase boundary

Phase 41 owns packed lowering for the SIMD operations NxIR can currently
express: v.2d integer add, v.2d FP add, equality masks, and d-lane swap. Broader
NEON decode and arbitrary vector permutations remain later compatibility work.
