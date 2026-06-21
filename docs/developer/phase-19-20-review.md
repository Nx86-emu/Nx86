# Phase 19-20 Review

Date: 2026-06-21

## Scope

This review covers Phases 19 and 20 from `SPEC.md`: a basic register allocator
for the single-block native path, and the first persistent AOT object format.
Both build directly on the Phase 16-18 native slice and stay within the same
narrow, host-independent envelope — only execute-after-reload is host-gated.

## Findings

- Phase 19 is in place: `nx86-regalloc` replaces the Phase 18 stack-everything
  placeholder with a deterministic linear-scan allocator over a single NxIR
  block. Each value lives from its definition to its last use and takes the
  lowest-index free pool register at definition; the six caller-saved registers
  `RDX, RSI, R8, R9, R10, R11` form the pool, and exhaustion spills to a fresh
  stack slot. `RDI` stays the `NativeBlockState` pointer and `RAX`/`RCX` remain
  scratch, so the generated leaf block still needs no register save/restore.
- `nx86-x64-v4` now consumes the allocation (register-resident values read/write
  guest state directly; spills reuse the stack-slot path; the frame is sized from
  the spill count) and lowers the three logical binary ops `AND`/`OR`/`XOR`
  alongside `Add`/`Sub`, with matching exact-byte emission added to
  `nx86-x64-asm`.
- Phase 20 is in place: `nx86-object` defines `NativeObject` and a compact
  little-endian `.nxo` format (magic + version header, guest mapping, stack-frame
  size, code bytes, trailing FNV-1a content hash). `to_bytes`/`from_bytes`
  serialize and validate (magic, version, exact length, hash); `write_to_path`/
  `read_from_path` wrap `std::fs`; `file_name` keys an object by guest entry
  address.
- `nx86-backend` bridges the in-memory lowering to disk via `native_object`,
  which builds a `NativeObject` from a `LoweredBlock` and its source function.
  The Linux-gated test closes the loop: lower → write → reload → allocate
  executable memory → run → match the interpreter, proving a compiled block
  persists across restart.
- The backend also gained a `NativeStatus::Unsupported` classification so that a
  valid-but-not-yet-lowerable program shape (multiple blocks, branches, unlowered
  ops) is reported as benign rather than as an error; the GUI surfaces it.

## Boundary Checks

- No title import, firmware, keys, commercial software path, HLE behavior, cache
  manager, dispatcher, JIT fallback, memory lowering, flags lowering, or branch
  lowering was added. Those remain later phases.
- `nx86-object` is pure logic plus `std::fs` with no `unsafe`; the validation
  hash is a dependency-free integrity/identity check, explicitly not a
  cryptographic one, left as the hook the Phase 21 cache "full check" upgrades.
- Native code execution stays host-gated: the object serialization, allocator,
  and lowering bytes are fully unit-tested on `aarch64-apple-darwin`; only
  execute-after-reload is behind `#[cfg(all(target_os = "linux", target_arch =
  "x86_64"))]` and runs in CI on `ubuntu-latest`.
- mmap/transmute `unsafe` remains confined to `nx86-jit`; `nx86-backend` keeps a
  single documented unsafe call-site for the bytes produced by `lower_tiny_block`.

## Verification

Passed locally on `aarch64-apple-darwin`:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets   # 112 tests, 0 failures
```

The Linux x86_64-only tests for calling and reloading generated code are compiled
behind `#[cfg(all(target_os = "linux", target_arch = "x86_64"))]` and were not
executed on the Apple Silicon development host; CI exercises them on
`ubuntu-latest`.
