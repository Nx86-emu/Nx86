# Phase 21-22 Review

Date: 2026-06-21

## Scope

This review covers Phases 21 and 22 from `SPEC.md`: managed persistent native
objects and the first multi-block native dispatcher. The implementation remains
within the synthetic integer-program boundary; it does not add an emergency JIT
or a Switch software execution path.

## Findings

- Phase 21 is in place: `nx86-cache` scans `.nxo` objects as the source of truth,
  persists an optional manifest snapshot, reports object count and size, and
  supports insert, load, remove, and clear operations.
- Shallow checks validate object identity from the header; full checks also
  validate the object's FNV-1a content hash. Cache clear removes every `.nxo`
  file, including corrupt objects that a normal scan cannot parse.
- The Library screen reports global CPU-object cache status and exposes cache
  clearing without treating the persisted manifest as an object.
- Phase 22 is in place: `nx86-x64-v4` lowers each supported NxIR block
  independently, unconditional branches publish the next guest PC, and
  `nx86-backend::Dispatcher` routes until halt, a missing block, or the step
  limit.
- The dispatcher can be built directly from a verified function or from
  persisted native objects. Cache-backed construction is unsafe because object
  integrity is not proof of trusted lowerer provenance.
- The runtime differential harness attempts dispatcher execution alongside the
  existing single-block path and compares supported native results with the
  interpreter.

## Boundary Checks

- No emergency JIT, runtime profile logging, conditional-branch lowering,
  native memory operations, native flags, title import, firmware, keys, HLE,
  graphics, or commercial-software path was added.
- Executable memory and generated-code calls remain inside `nx86-jit`; the
  backend documents the `NativeBlockState` ABI at its unsafe call site.
- `Dispatcher::from_objects` requires the caller to establish that cached bytes
  came from Nx86's trusted lowerer and were not forged after persistence. The
  current content hash detects accidental corruption only.
- Native execution stays gated to Linux x86_64. Host-independent lowering,
  allocation, serialization, cache management, and error paths run on the Apple
  Silicon development host.

## Verification

Passed locally on `aarch64-apple-darwin`:

```sh
actionlint .github/workflows/ci.yml .github/workflows/linux-x86_64-v4.yml
git diff --check
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets   # 131 tests, 0 failures
cargo build --workspace
```

The artifact workflow now applies x86-64-v4 and static-CRT flags only to an
explicit `x86_64-unknown-linux-gnu` target. This keeps those flags off host
proc-macros, addressing the previous `bytemuck_derive` build failure. Actual
Linux native execution and artifact production remain pending remote CI because
the local host is Apple Silicon and its Docker daemon is unavailable.
