# Phase 30 Review

Date: 2026-06-22

## Scope

This review covers Phase 30 from `SPEC.md`: direct memory lowering, a memory
base register, fast loads/stores, checked fallback, and synthetic fastmem tests.
The authoritative semantics are SPEC §16 and §25.

## Findings

- `nx86-vmm` now backs Linux guest pages directly inside the reserved 64 GiB
  arena. A 16 MiB byte-per-page table publishes only read/write eligibility.
  Full-span validation remains in the checked APIs, preserving the no-partial-
  write invariant. Logical memory has no native view and always uses fallback.
- `NativeBlockState` appends fastmem base/table pointers and a slowmem callback
  contract after the existing CPU fields, so their offsets and version-1
  CPU-only `.nxo` objects remain compatible. Persisted memory objects did not
  exist before this phase because the lowerer rejected memory operations.
- The native ABI reserves callee-saved `r12`-`r15`; allocator registers and both
  scratch registers are preserved around helper calls. Spill offsets account
  for saved registers. Normal, fault, and chain exits restore the same frame.
- The assembler now supports indexed operands, 32-bit memory operations,
  byte-to-64-bit loads, shifts/masks/tests, conditional branches, indirect
  calls, and explicit register saves. Encoding remains host-independent.
- The lowerer emits fast and fallback paths for I32/I64 `Load`/`Store`, plus the
  `Trunc`/`ZeroExtend` operations produced by word loads/stores. Inline checks
  occur before the permission-table lookup, preventing out-of-bounds host reads;
  cross-page accesses always use the checked helper.
- The helper ABI returns status through `NativeBlockState`; reads return their
  value in a dedicated slot. VMM faults retain their typed source and guest PC.
  Generated blocks never dereference a missing arena or permission table.
- `Dispatcher::run_in`, `run_tiny_native_block_in`, and
  `run_dispatched_function_in` attach memory without breaking existing
  memoryless callers. Synthetic differentials compare native memory as well as
  CPU state.

## Boundary and safety checks

- New unsafe code is confined to Linux arena page operations, the existing
  generated-code boundary, and two callback pointer recoveries whose lifetime
  is bounded by a synchronous native call.
- Fastmem admits only in-arena, single-page accesses with the requested page
  permission. Every other access takes the helper before a host pointer is
  dereferenced.
- Host pages are never writable and executable solely for fastmem purposes.
  Remapping zeroes pages; unmapping applies `PROT_NONE` before discarding data.
- No slowmem counters, profile events, Native Coverage scoring, memory
  mirroring, concurrent mapping mutation, or SMC behavior is claimed here.
- No proprietary data, firmware, keys, game content, `unwrap`, `todo`, or `dbg`
  was introduced.

## Verification

Passed locally on `aarch64-apple-darwin`:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo check -p nx86-backend -p nx86-runtime --all-targets --target x86_64-unknown-linux-gnu
cargo clippy -p nx86-backend -p nx86-runtime --all-targets --target x86_64-unknown-linux-gnu -- -D warnings
```

The Linux-only integration test compiles in the cross-target gates. Actual
x86-64 generated-code execution remains a Linux x86_64 runtime check because the
Apple Silicon host cannot execute it and the local Docker daemon was unavailable.
