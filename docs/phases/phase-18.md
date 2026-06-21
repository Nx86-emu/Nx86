# Phase 18: First Native Block

Phase 18 adds the first native x86_64 block path. `nx86-x64-v4` lowers a
verified single-block NxIR integer subset into x86_64 bytes, using stack slots
for SSA values and a `NativeBlockState` layout for guest registers, SP, PC,
NZCV bits, and halt state.

`nx86-backend` orchestrates native execution: lower the verified NxIR function,
allocate executable memory where supported, call the block, convert the native
state back to `CpuState`, and compare it with the AArch64 interpreter result.
`nx86-runtime` now lifts and optimizes once, then runs both the NxIR evaluator
and the native attempt from the same function. The GUI Tests screen shows a
compact native x86_64 status and assembler dump next to the NxIR dump.

This phase intentionally supports only the first tiny integer path:
single-block NxIR with `Const`, `GetReg`, `SetReg`, `Binary Add/Sub i64`, and
`Halt`/`Return`. Branches, memory ops, flags, logical ops, register allocation,
object caching, dispatcher integration, and real software execution remain
later phases.

## Exit Criteria

- The synthetic ARM64 add program lowers to native x86_64 bytes.
- Linux x86_64 can execute the generated block and match the interpreter.
- Unsupported hosts report native execution as unavailable while retaining the
  assembler dump and NxIR differential result.
