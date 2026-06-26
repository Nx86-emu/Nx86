# Phase 42: Hot/Cold Splitting

Phase 42 adds profile-guided native block layout without changing dispatcher
keys or guest-visible control flow.

## What it does

- **Profile hot blocks** - `hot_cold_layout` derives per-block heat from
  `JitBlock` and `BranchTarget` profile records.
- **Split cold paths** - unobserved blocks are marked `CodeSection::Cold` and
  moved after observed hot blocks in the emitted block list.
- **Layout hot loops** - the layout follows the hottest observed successor chain
  from the entry block, then appends remaining hot blocks by heat.
- **Update native mapping** - `lower_function_with_profile` lowers blocks in
  profile order while preserving each block's guest entry PC as the dispatcher
  key and native mapping identity.

## Design

The first block remains first for stable entry behavior. Profile data may move a
hot successor ahead of cold original-order blocks, but branch target resolution
continues to use the full function entry table, so reordering cannot change
guest branch semantics.

## Phase boundary

Phase 42 owns block-order layout decisions and hot/cold section metadata. It
does not yet persist section metadata into `.nxo` files or perform linker-level
text-section splitting.
