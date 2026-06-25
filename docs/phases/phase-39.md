# Phase 39: FP Scalar Support

Phase 39 adds a narrow scalar FP path for the synthetic AArch64 programs used by
Nx86's current differential tests.

## What it does

- **FP registers** - `CpuState` exposes 64-bit SIMD/FP lane helpers and scalar
  `f64` helpers on top of the existing 32-register `u128` vector file.
- **Decoder** - scalar double `fmov #1.0/#2.0`, `fadd`, `fsub`, `fmul`,
  `fdiv`, and `fcmp` decode with disassembly and `InstructionClass::FloatingPoint`.
- **NxIR** - side-effecting `FpMoveImmediate`, `FpScalarBinary`, and
  `FpCompare` ops preserve architectural FP register effects without widening
  integer SSA values.
- **Runtime** - the interpreter and NxIR evaluator execute scalar FP arithmetic
  with Rust `f64` semantics and materialize NZCV for `fcmp`.
- **FPCR/FPSR basics** - `NativeBlockState` now carries `fpcr` and `fpsr`, and
  `CpuState` continues to serialize and compare them through register names.
- **Synthetic fixture** - `tests/synthetic/scalar-fp.toml` covers scalar FP
  arithmetic, divide, compare, and exact register bit patterns.

## Design

Phase 39 keeps FP operations side-effecting at the IR level. That mirrors the
Phase 35 barrier approach: preserve architectural information first, then
broaden optimizer and native lowering contracts in later phases.

## Phase boundary

Phase 39 owns scalar double decode, lift, interpreter/evaluator execution,
FP/SIMD lane helpers, and synthetic exactness tests. It does not own full
AArch64 FP immediate expansion, single precision, FP exceptions, host MXCSR/FPCR
mapping, or native lowering for `fcmp`.
