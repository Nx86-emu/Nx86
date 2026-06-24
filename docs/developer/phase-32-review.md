# Phase 32 Review

Date: 2026-06-23

## Scope

This review covers Phase 32 from `SPEC.md`: memory mirroring design,
release-mode enablement, debug-mode disablement, and correctness tests. The
authoritative semantics are SPEC §25.3.

## Findings

- `GuestMemory` gains two new fields: `mirrors: BTreeMap<u64, u64>` (mirror
  page base → canonical page base) and `debug_mode: bool`. Constructors
  `new()`, `new_with_mode(debug_mode)`, and `new_logical()` initialize both
  fields.
- `VmmFault` gains `MirrorSourceUnmapped { address }` and
  `MirrorTargetMapped { address }` variants for precondition violations.
- `mirror_page(mirror, canonical, permissions)` validates that the canonical
  page is mapped and the mirror address is free, then registers the mapping.
  In release mode, mirror pages have `data: None` (shared via redirect). In
  debug mode, mirror pages get an independent copy of the canonical page's
  data.
- `resolve_address()` translates mirror addresses to canonical addresses
  before data access. `copy_out` and `write` use the resolved address for
  arena/logical data access while keeping permission checks on the original
  mirror address. In logical mode (non-Linux), mirror reads resolve to the
  canonical page's `data` field; mirror writes resolve to the canonical
  page's `data` field.
- `unmap_mirror(mirror)` removes the mirror from `pages`, `mirrors`, and the
  reverse-dependency `mirror_sources` map, then zeroes the fastmem eligibility
  byte. `unmap_page` on a canonical page is blocked if active mirrors depend on
  it, preventing dangling redirects.
- `mirror_target(address)` returns `Some(canonical)` for mirrors, `None`
  otherwise. `is_mirror(address)` checks the mirrors map. `is_debug_mode()`
  exposes the flag.
- A reverse-dependency map `mirror_sources: BTreeMap<u64, Vec<u64>>` tracks
  which mirrors depend on each canonical page. `unmap_page` checks this map
  and returns `MirrorTargetMapped` if the page has active mirrors.
- Mirror permissions are validated against the canonical page's permissions:
  a mirror cannot grant read/write/execute that the canonical does not have.
  This prevents arena-level permission violations on Linux where the physical
  page's mprotect flags are set by the canonical's permissions.
- Mirror pages are always fastmem-ineligible (byte 0). Canonical pages retain
  their fastmem eligibility unchanged.
- `CompilerConfig` gains `debug_memory_mirrors: bool` (default `false`).
  The config round-trips through TOML and is checked in the defaults test.
- The `slowmem_read` and `slowmem_write` callbacks now emit `"mirror"` as the
  reason code when `memory.is_mirror(GuestAddress(address))` is true. The
  `FaultReport` match handles `MirrorSourceUnmapped` and `MirrorTargetMapped`.
- 18 new unit tests cover: shared backing, bidirectional writes, unmap,
  precondition violations, independent permissions, permission violations,
  debug-mode independence, debug-mode initial copy, release-mode sharing,
  fastmem ineligibility, `mirror_target`, `is_mirror`, canonical unmap
  blocking, mirror cleanup after unmap, source tracking cleanup,
  mirror-of-mirror rejection, and mirror permission ceiling enforcement.

## Boundary and safety checks

- No new `unsafe` code was introduced. The existing `unsafe` in the slowmem
  callbacks is unchanged; `is_mirror` is called through the same `NonNull`
  pointer lifetime as the callbacks.
- Mirror access always goes through the slowmem callback path (no fastmem for
  mirrors). This is a conscious design choice; arena-level sharing via
  `memfd_create` is a future optimization.
- No `unwrap`, `todo`, `dbg`, or new `unsafe` was introduced.
- No proprietary data, firmware, keys, game content, or personal data was
  introduced.

## Verification

Passed locally on `aarch64-apple-darwin`:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
```

All 215 tests pass (up from 182 before Phase 32). The 18 new mirror tests in
`nx86-vmm` verify shared backing, debug-mode independence, permission
isolation, precondition enforcement, fastmem eligibility, mirror
introspection, canonical unmap blocking, mirror source cleanup,
mirror-of-mirror rejection, and mirror permission ceiling enforcement. The existing dispatch and synthetic tests exercise the updated
slowmem callback paths through the differential testing oracle.
