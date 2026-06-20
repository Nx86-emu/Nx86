# Phase 8: AArch64 Decoder Skeleton

Phase 8 replaces the placeholder decoder with a narrow AArch64 decoder for
synthetic tests. The supported subset is 64-bit MOVZ immediate as MOV,
ADD/SUB immediate, unconditional B immediate, and SVC.

## Exit Criteria

- MOV, ADD, SUB, B, and SVC decode from little-endian instruction words.
- Decoded instructions expose address, raw bytes, instruction class, operands,
  disassembly, and structured decode errors.
- The GUI Tests screen can show decoded synthetic instructions.
