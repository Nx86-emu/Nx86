# Phase 7: CpuState and Guest State Model

Phase 7 adds the guest CPU state model in `nx86-core::guest`. It tracks 31
general registers, SP, PC, NZCV, FP/SIMD registers, FPCR/FPSR, thread metadata,
halt state, debug serialization, text dumps, and expected-register comparison.

## Exit Criteria

- CpuState can be created.
- CpuState can be serialized and dumped for debugging.
- Synthetic tests can compare expected registers against CpuState.
