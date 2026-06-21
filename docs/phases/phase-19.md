# Phase 19: Basic Register Allocator

Phase 19 replaces the Phase 18 placeholder allocator (every SSA value forced to a
stack slot) with a real one. `nx86-regalloc` is now filled in: a deterministic
linear-scan allocator over a single NxIR block. Each value lives from its
definition to its last use and is given the lowest-index free pool register at
definition; when the pool is exhausted the value spills to a fresh stack slot.

The allocatable pool is the six caller-saved x86_64 registers `RDX, RSI, R8, R9,
R10, R11`. `RDI` stays the `NativeBlockState` pointer for the whole block, RSP/RBP
frame the stack, and RAX/RCX remain fixed scratch for binary-operand staging and
spill loads/stores. Because every pool register is caller-saved, the generated
leaf block still needs no register save/restore in its prologue.

`nx86-x64-v4` consumes the allocation: register-resident values read and write
guest state directly, spilled values reuse the existing stack-slot mechanism, and
the stack frame is now sized from the spill count rather than the value count.
This phase also adds lowering for the three logical binary ops — `AND` (`0x21`),
`OR` (`0x09`), `XOR` (`0x31`) — alongside the existing `Add`/`Sub`, so the
supported single-block subset is now `Const`, `GetReg`, `SetReg`,
`Binary Add/Sub/And/Or/Xor (i64)`, and `Halt`/`Return`.

Branches, multi-block control flow, memory ops, flags lowering, object caching,
and dispatcher integration remain later phases.

## Exit Criteria

- Multi-instruction single-block NxIR functions allocate to real registers and,
  on Linux x86_64, execute and match the interpreter.
- A block whose live-value count exceeds the register pool spills correctly and
  still matches the interpreter (Linux x86_64).
- The allocator is pure logic and fully unit-tested on the Apple Silicon dev
  host; native execution stays host-gated.
