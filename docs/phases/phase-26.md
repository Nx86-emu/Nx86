# Phase 26: CFG Recovery v0

Phase 26 begins recovering program structure from raw guest code. Where the
lifter builds a basic-block CFG over a contiguous, pre-decoded instruction slice
for a function whose boundary is already known, recovery *derives* that
structure: from one or more entry PCs it decodes on demand, follows direct
branch and fall-through successors through a worklist, and reports the reachable
blocks, the per-function block sets, and any unresolved exits.

This is pure analysis — no native code generation — so it builds and tests on
every host, including the Apple Silicon dev host.

## What it does

`recover::recover_cfg(&CodeView, &[entry])` runs recursive-descent recovery:

- **Recursive traversal** — a worklist of block-entry PCs decodes reachable code
  on demand through `nx86_arm64_decode::decode_instruction`. Unreachable bytes
  (e.g. dead code after an unconditional branch) are never decoded.
- **Direct branch following** — `B` is followed to its target; `B.cond` is
  followed to both its target and its fall-through; `SVC` is a program exit.
  Indirect, undecodable, or out-of-range successors are recorded as
  `EdgeKind::Unresolved` and not followed.
- **Basic block table** — instructions are grouped into blocks split at every
  leader (entry, branch target, or conditional fall-through), keyed by start
  address in a `BTreeMap` for a deterministic table. A branch into the middle of
  an already-recovered run splits that run at the new leader.
- **Function candidate discovery** — each distinct entry PC seeds one
  `RecoveredFunction` covering the blocks transitively reachable from it. v0
  recovers a single function per entry; `BL`/call-driven discovery of additional
  functions arrives with broader decoder coverage (the seam is marked in
  `explore`).
- **CFG display** — `RecoveredCfg` implements `Display`, rendering each function
  and its blocks (`start..end`, instruction count, and resolved successors) as
  deterministic text for the Inspector (Phase 27) and the GUI to surface.

## New types

- `CodeView<'a>` — a read-only, base-relative window over guest code; reads one
  4-byte word at a time so out-of-range or misaligned addresses are unresolved
  rather than panics.
- `EdgeKind` — `Fallthrough`, `DirectBranch`, `CondBranch { taken, not_taken }`,
  `Exit`, `Unresolved`.
- `RecoveredBlock` — `start`, `end`, `instruction_count`, `terminator`,
  resolved `successors`.
- `RecoveredFunction` — an `entry` and the sorted `block_starts` reachable from
  it.
- `RecoveredCfg` — function candidates plus the block table.
- `RecoverError` — `UnalignedCode`, `NoEntries`, `UndecodableEntry`.

## Exit Criteria

- Synthetic programs produce a CFG: straight-line, forward branch (with dead
  fall-through pruned), conditional diamond that reconverges, backward-branch
  loop (traversal terminates), and a branch into a run that splits the block.
- Recovered block starts agree with the lifter's existing CFG construction for a
  fully reachable, contiguous program.
- `Display` renders a deterministic CFG.
