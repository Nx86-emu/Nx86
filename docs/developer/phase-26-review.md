# Phase 26 Review

Date: 2026-06-21

## Scope

This review covers Phase 26 from `SPEC.md`: recursive traversal, direct branch
following, function-candidate discovery, a basic-block table, and CFG display.
The exit criterion is that synthetic programs produce a CFG.

## Findings

- Recovery lives in a new `recover` module in `nx86-arm64-lift`
  (`crates/nx86-arm64-lift/src/recover.rs`), re-exported from the crate root. It
  reuses the existing `nx86-arm64-decode` decoder and stays a sibling of the
  lifter rather than a rewrite of it.
- `recover_cfg(&CodeView, &[entry])` runs a two-phase pass: recursive descent
  (`explore`) collects reachable instructions and block leaders; `build_blocks`
  groups them into a `BTreeMap`-keyed block table, splitting at every leader.
- Direct branches (`B`) and conditional branches (`B.cond`, both target and
  fall-through) are followed; `SVC` is a program exit. Indirect, undecodable, or
  out-of-range successors are recorded as `EdgeKind::Unresolved` and not
  followed.
- A branch into the middle of an already-recovered run adds a leader, so block
  construction splits that run at the boundary — the same leader invariant the
  lifter's `compute_block_starts` relies on. The fall-through-into-decoded-code
  case always lands on a leader, so predecessor edges resolve cleanly.
- Function discovery seeds one `RecoveredFunction` per distinct entry covering
  the blocks transitively reachable from it. The `BL`/call seam for additional
  function candidates is marked in `explore`; v0 recovers a single function.
- `RecoveredCfg` implements `Display` for a deterministic textual CFG that
  Phase 27 (Inspector) and the GUI can render unchanged.

## Boundary Checks

- Recovery is pure analysis over synthetic test bytes: no native code
  generation, no `unsafe`, and no new mmap/transmute boundaries.
- `CodeView` reads bounds- and alignment-checked words, so out-of-range or
  misaligned addresses yield `Unresolved`/`None` rather than panics.
- No game dumps, firmware, or copyrighted blobs are imported or embedded; the
  decoder operates on caller-supplied bytes only.
- No `unwrap`/`todo`/`dbg` in the module; recovery never panics on malformed
  input (it reports `RecoverError` or `Unresolved`).

## Verification

Passed locally on `aarch64-apple-darwin`:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets   # 167 tests, 0 failures
```

The seven new tests in `recover.rs` exercise: a straight-line single-exit block;
a forward branch that prunes its dead fall-through; a conditional diamond that
reconverges; a backward-branch loop whose traversal terminates; a branch into a
run that splits the block; a deterministic `Display` snapshot; and a consistency
check that recovered block starts match the lifter's CFG construction for a
fully reachable program. Recovery is host-independent, so these run on the dev
host with no Linux-only paths.
