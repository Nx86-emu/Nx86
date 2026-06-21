# Phase 22: Dispatcher

Phase 22 lets execution span more than one native block. Each NxIR block is
lowered to its own native block keyed by its guest entry PC, and a dispatcher
routes between them by guest PC.

`nx86-x64-v4` gains `lower_function`, which lowers every block of a verified
function into a `LoweredFunctionBlock { entry_pc, lowered }`. The block-exit
protocol reuses `NativeBlockState`: the prologue clears the halted flag, an
unconditional `Branch` sets PC to the target block's guest entry and leaves the
flag clear (so the dispatcher routes onward), and `Halt` sets PC past the
terminator and raises the flag (so the loop stops). `block_entry_pc` derives a
block's key from its first instruction's guest address. Inter-block dataflow
already travels through the guest register file (the lifter keeps SSA values
block-local), so each block lowers independently. `lower_tiny_block` keeps
rejecting branches — a lone block has no sibling to reach. Conditional branches
and `Return` remain unlowered (native flags are a later phase).

`nx86-backend` adds the `Dispatcher`: a registry of native blocks keyed by guest
entry PC, built either `from_function` (lower in place) or `from_objects` (load
cached `.nxo` objects), and a `run` loop that looks up the current PC, calls the
block, and continues until a block halts. A guest PC with no registered block is
returned as `DispatchExit::MissingBlock` — the seam the Phase 23 emergency JIT
fills — and a step budget guards against runaway loops. `run_dispatched_function`
is the multi-block analogue of `run_tiny_native_block`, classifying the result
against the interpreter. The differential harness in `nx86-runtime` now runs this
dispatcher cross-check alongside the single-block attempt for every synthetic
test.

The emergency JIT (compiling a missing block on demand) and runtime profile
logging remain later phases.

## Exit Criteria

- A multi-block synthetic program (a branch routing between two blocks) executes
  through the dispatcher and, on Linux x86_64, matches the interpreter; the
  single-block path reports the same program as `Unsupported`.
- Blocks persisted as `.nxo` objects load from the cache and dispatch correctly
  (Phase 21 ↔ Phase 22 tie-in).
- A guest PC with no registered block is reported as `MissingBlock` rather than
  crashing.
- Per-block lowering and branch routing are pure logic and unit-tested on the
  Apple Silicon dev host; only execution stays host-gated to Linux x86_64.
