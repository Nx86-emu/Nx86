# Phase 16: x86_64 Assembler Skeleton

Phase 16 fills `nx86-x64-asm` with the first internal assembler API. It emits
trusted x86_64 machine-code bytes for a deliberately small instruction set:
64-bit `mov`, `add`, `sub`, `cmp`, `jmp`, labels, stack prologue/epilogue
helpers, register operands, memory operands, and a human-readable dump.

The assembler is not a general-purpose external assembler. It is an internal
byte emitter for later Nx86 backend phases, with exact byte tests for the
encodings the Phase 18 tiny-block lowerer needs.

## Exit Criteria

- The assembler emits deterministic bytes for basic integer operations, memory
  operands, labels, and stack-frame helpers.
- Label fixups reject unresolved labels instead of producing partial code.
- A generated `mov rax, imm; ret` function can be called through the Phase 17
  executable-memory layer on Linux x86_64.
