# Phase 29 Review

Date: 2026-06-21

## Scope

This review covers the Phase 29 software chain cache, patchable unconditional
branch exits, guarded Linux x86_64 native patching, invalidation, configuration,
statistics, and documentation. The authoritative roadmap is `SPEC.md` Phase 29,
with patch-site and invalidation constraints from SPEC §23.3 and §26.

## Findings

- The assembler emits a fixed five-byte `ret` + `nop` chain slot after complete
  stack teardown. The lowerer retains the successor-PC store and records the slot
  offset, original bytes, successor, and patch state. The register allocator
  excludes `rdi`, preserving the state-pointer chain ABI.
- Dispatcher blocks now use a guest-PC `HashMap`. A host-independent chain cache
  installs direct edges after two observations, tracks reverse software edges,
  supports pre-run and fallible runtime debug disable, and exposes per-run plus
  cumulative statistics. Runtime disable restores native exits before reporting
  success; missing successors are never cached.
- Native patching requires the off-by-default `native-patch-chaining` feature,
  Linux x86_64, and the off-by-default runtime preference. It accepts only
  forward unconditional Branch→Branch edges with compiled source and target
  metadata and a rel32 displacement representable as `i32`. Other cases retain
  the dispatcher fallback. Canonical slot bytes, ownership, size, and bounds are
  revalidated before patching; safe failures are retried on later software hits.
- `nx86-jit` validates patch ranges and changes the full mapping RX→RW→RX, never
  RWX. Failure to make a mapping writable leaves the original RX code intact and
  falls back; failure to restore RX quarantines the mapping and is fatal.
- Invalidation tracks native incoming edges, restores original slot bytes for
  incoming and outgoing patches, then clears corresponding software chains.
  Reverse-edge state is removed only after restoration succeeds, keeping failed
  invalidation retryable. Native cycles remain disallowed so patched execution
  cannot loop indefinitely outside `max_steps`.
- Cached `.nxo` and emergency-JIT blocks use software chaining only because the
  v0 object format does not persist patch metadata.

## Boundary checks

- New unsafe operations are confined to the existing `nx86-jit` executable-memory
  boundary and two narrowly documented single-threaded backend call sites.
- rel32 arithmetic uses checked `usize` addition followed by `i128` subtraction
  and checked `i32` conversion. Patch sizes are verified before rewriting.
- No proprietary data, firmware, keys, game content, `unwrap`, `todo`, or `dbg`
  was introduced.

## Verification

Passed locally on `aarch64-apple-darwin`:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo build -p nx86-backend --features native-patch-chaining
cargo clippy -p nx86-backend --features native-patch-chaining --all-targets -- -D warnings
cargo check -p nx86-backend --features native-patch-chaining --all-targets --target x86_64-unknown-linux-gnu
cargo clippy -p nx86-backend --features native-patch-chaining --all-targets --target x86_64-unknown-linux-gnu -- -D warnings
```

The Linux target checks compile the guarded patch implementation and its tests.
Actual native execution, `/proc/self/maps` permission verification, and the
forward-chain integration test remain Linux x86_64 runtime validation; they
cannot execute on the Apple Silicon development host.
