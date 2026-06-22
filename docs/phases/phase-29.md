# Phase 29: Block Chaining v0

Phase 29 adds two complementary block-routing layers to the native dispatcher:
an always-available software chain cache and an experimental native exit-patch
backend. Correctness never depends on native patching; an unpatched block exits
with `ret` and continues through the dispatcher.

## What it does

- **Software chaining and table lookup** — the dispatcher stores native blocks
  in an O(1) guest-PC table, counts observed edges, and installs a direct chain
  after two observations. Chaining can be disabled before or during execution
  for debug runs; runtime disable restores native exits before clearing the
  software cache. Outcomes expose per-run chain statistics.
- **Patchable branch exits** — an unconditional `Branch` ends with a five-byte
  slot containing `ret` plus four `nop` bytes. The lowerer records its offset,
  original bytes, successor PC, and patch state.
- **Guarded native chaining** — a hot forward `Branch`→`Branch` edge may replace
  the slot with `jmp rel32`. This requires the `native-patch-chaining` Cargo
  feature, Linux x86_64, and the runtime `native_patch_chaining` preference;
  all three gates default to off except the platform constraint.
- **W^X patching** — `nx86-jit` changes the complete mapping from RX to RW,
  writes the slot, and restores RX. It never maps writable and executable at the
  same time. A failed RX restoration quarantines the mapping and stops dispatch.
- **Invalidation** — the dispatcher tracks incoming native edges and restores
  their original bytes before a target can be replaced. Invalidation also clears
  the block's outgoing edges and all corresponding software chains. Reverse-edge
  bookkeeping remains intact until restoration succeeds, so a failed unpatch can
  be retried safely.

Before any native rewrite, the backend verifies that patch metadata belongs to
the source block, describes an unconditional exit, contains the canonical
`ret` + `nop` bytes, uses the required size, and lies within the mapping. Safe
patch failures retain software routing and are retried when the hot edge is hit
again.

The chain ABI keeps the `NativeBlockState` pointer in `rdi`, fully unwinds each
block's stack frame before its exit slot, and retains every successor-PC store.
Native v0 only patches forward edges into blocks that themselves have a chain
exit. This prevents native cycles from bypassing the dispatcher step budget and
prevents a chained call from hiding the PC used to classify a halt reason.

## Deferred work

- conditional, `CondBranch`, and `Guard` exit patching
- cyclic/back-edge chaining with explicit safepoints and step accounting
- computed and indirect target table chaining
- multi-threaded patch synchronization and non-x86 instruction-cache handling
- persisting patch metadata and installed chains in `.nxo` objects
- SMC-driven invalidation wiring (Phase 33)
- GUI exposure of the experimental runtime flag
- workload-driven hotness-threshold tuning

## Exit criteria

A Linux x86_64 feature-gated integration test repeatedly executes a bounded
three-block forward path. The second run installs a native Branch→Branch patch,
the next run enters the patched chain and still matches the expected guest state,
and invalidation restores dispatcher routing.
