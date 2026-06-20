# Phase 9: Tiny Interpreter

Phase 9 adds a tiny interpreter in `nx86-runtime` for the Phase 8 instruction
subset. It updates registers and PC, handles unconditional branches, treats SVC
as the synthetic halt boundary, and reports expected-register mismatches.

## Exit Criteria

- Synthetic integer add programs execute.
- Branches update PC.
- SVC halts execution.
- Expected register values are compared against the final CpuState.
