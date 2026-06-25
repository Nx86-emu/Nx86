# Phase 40: SIMD/NEON v0

Phase 40 adds the first basic NEON path over 128-bit vector registers.

## What it does

- **Vector model** - `CpuState` lane helpers expose v0..v31 as two 64-bit lanes
  while retaining the existing `u128` register representation.
- **Decoder** - `add vN.2d, vN.2d, vN.2d` and `fadd vN.2d, vN.2d, vN.2d`
  decode with `InstructionClass::Simd`.
- **NxIR** - `VectorBinary` records integer and FP vector add operations as
  side-effecting SIMD register operations.
- **Runtime** - the interpreter and NxIR evaluator execute integer lane adds
  with wrapping `u64` semantics and FP lane adds with `f64` semantics.
- **x86_64-v4 lowering** - native state now carries all vector lanes. The lowerer
  emits integer lane adds with general registers and FP lane adds with scalar
  SSE2 double operations.
- **Debug validation** - unit tests cover decode, lift, IR metadata, evaluator
  agreement, assembler SSE emission, native state ABI offsets, and native vector
  lowering dumps.
- **Synthetic fixture** - `tests/synthetic/neon-basic.toml` covers basic NEON
  integer and FP vector add.

## Design

The v0 vector lowering is intentionally lane-oriented. It proves the native ABI
can preserve SIMD state and that vector ops can pass through the same synthetic
differential surface as scalar code. Phase 41 still owns advanced vector
register allocation, masks, shuffles, compares, and spills.

## Phase boundary

Phase 40 owns v.2d integer add, v.2d FP add, vector lane state, runtime
execution, and first native lowering. It does not own broad NEON decode,
advanced x86_64-v4 vector lowering, SIMD comparisons, shuffle lowering, or
vector spill strategy.
