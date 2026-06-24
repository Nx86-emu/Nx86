# Phase 32 Audit: Memory Mirroring

Date: 2026-06-24
Auditor: MiMo V2.5 Pro
Scope: all diffs in Phase 32 (crates/nx86-vmm, crates/nx86-core, crates/nx86-backend)

## Summary

**No open bugs found.** Three bugs were identified and fixed during
implementation. The final code is correct, passes all checks (fmt, clippy,
215 tests), and maintains backward compatibility.

## Files changed

| File | Lines added | Lines removed | Nature |
|------|------------|---------------|--------|
| `crates/nx86-vmm/src/lib.rs` | ~480 | ~30 | Mirror logic + 18 tests |
| `crates/nx86-core/src/config.rs` | 6 | 0 | `debug_memory_mirrors` field |
| `crates/nx86-backend/src/lib.rs` | ~15 | ~8 | Mirror reason + fault arms |
| `docs/phases/phase-32.md` | new | — | Phase design doc |
| `docs/developer/phase-32-review.md` | new | — | Implementation review |

## Bugs found and fixed during implementation

### BUG-1: Dangling mirrors after canonical unmap (severity: high)

`unmap_page` on a canonical page did not check for or clean up dependent
mirrors. After `unmap_page(0x1000)` with `mirrors[0x2000] = 0x1000`, reads
through 0x2000 would hit unmapped arena memory (Linux: potential segfault;
logical: silent Unmapped error).

**Fix:** Added `mirror_sources: BTreeMap<u64, Vec<u64>>` reverse-dependency
map. `unmap_page` checks `mirror_sources` and returns `MirrorTargetMapped` if
the page has active mirrors. `unmap_mirror` cleans up `mirror_sources`.

### BUG-2: Mirror permissions could exceed canonical permissions (severity: high)

On Linux, the arena page is mprotect'd with the canonical's permissions. A
READ_WRITE mirror of a READ-only canonical would pass the permission check in
`write()` but segfault in `arena.copy_in()` (writing to a PROT_READ page).

**Fix:** `mirror_page` validates that requested permissions are a subset of the
canonical's permissions. Returns `VmmFault::Permission` on violation.

### BUG-3: Mirror-of-mirror bypass (severity: medium)

If 0x2000 mirrors 0x1000, then 0x3000 mirrors 0x2000, `resolve_address(0x3000)`
would resolve to 0x2000 (still a mirror) in a single lookup. Reads would go
through the wrong path (mirror's `data: None` → empty/error).

**Fix:** `mirror_page` rejects mirrors where the canonical is itself a mirror.
`resolve_address` chains up to 4 levels as defense-in-depth.

## Code review findings (no bugs)

### FINDING-1: Silent no-op on non-mirror pages with `data: None` (severity: very low)

In `copy_out` and `write`, the final else branch for pages with `data: None`
and no arena only handles mirrors explicitly. A non-mirror page with `data: None`
in logical mode would silently skip the data operation. This cannot occur in
practice because logical-mode pages always have `data: Some(...)`, and Linux
arena pages always have `data: None` (handled by the arena branch). The mirror
branch is the only path where `data: None` + no arena is reachable.

**Assessment:** Defensive coding, not a bug. No change needed.

### FINDING-2: `is_mirror` returns false for debug-mode mirrors (severity: none)

In debug mode, mirrors get independent data copies and are not registered in
the `mirrors` map. `is_mirror()` returns false and `mirror_target()` returns
None. This is correct: debug-mode mirrors are independent copies with no
redirect relationship. The backend's "mirror" reason code only fires for
release-mode mirrors (which go through the slowmem path).

**Assessment:** Intentional design, not a bug.

### FINDING-3: `unmap_mirror` is a no-op on non-mirror addresses (severity: very low)

`unmap_mirror` silently succeeds even if the address is not a mirror. It removes
the page from `pages` (if present) and zeroes fastmem, but does not return an
error. This matches `unmap_page`'s behavior (no error on already-unmapped pages)
but could mask caller bugs.

**Assessment:** Consistent with existing API contract. Consider adding a check
in a future cleanup pass.

### FINDING-4: `arena.unmap_page` called on never-mapped mirror (severity: none)

`mirror_page` calls `arena.unmap_page(mirror_base)` on an address that was
never `map_page`'d. On Linux this is `mprotect(PROT_NONE)` + `madvise(MADV_DONTNEED)`
on an already-PROT_NONE page (harmless no-op). On non-Linux it's a no-op.

**Assessment:** Defensive but unnecessary. Could be removed for clarity.

### FINDING-5: `mirror_sources` leaves empty Vec entries after unmap (severity: none)

`unmap_mirror` uses `retain` to remove the mirror from the sources list but
does not remove the `mirror_sources` entry when the list becomes empty. This
leaves `canonical_base → []` entries in the map. Not a correctness issue (the
`unmap_page` guard checks `is_empty`), and the map is small.

**Assessment:** Cosmetic. Could add a post-retain cleanup if desired.

### FINDING-6: Backend `slowmem_write` borrows `NonNull` twice (severity: none)

The write callback creates `memory_ref` (immutable via `as_ref()`) to check
`is_mirror`, then creates `memory` (mutable via `as_mut()`) for the actual
write. Both borrows are to the same `NonNull<GuestMemory>`. This is safe because:
- `NonNull` is `Copy`, so `memory` is a fresh copy
- `memory_ref` is only used for `is_mirror` (read-only, no mutation)
- The mutable borrow through `memory` starts after `memory_ref` is no longer used
- The SAFETY comment documents this

**Assessment:** Correct. No aliasing violation.

## Test coverage

18 new unit tests in `crates/nx86-vmm/src/lib.rs`:

| Test | What it covers |
|------|---------------|
| `page_permissions_returns_mapped_page` | `page_permissions()` on mapped page |
| `page_permissions_returns_none_for_unmapped` | `page_permissions()` on unmapped page |
| `mirror_page_shares_backing` | Write canonical → read mirror |
| `mirror_write_visible_at_canonical` | Write mirror → read canonical |
| `mirror_unmap_removes_mirror` | Unmap then read faults |
| `mirror_requires_mapped_canonical` | Unmapped canonical rejected |
| `mirror_rejects_already_mapped_target` | Already-mapped target rejected |
| `mirror_permissions_independent` | Mirror can have subset permissions |
| `mirror_permission_violation_faults` | Write to READ mirror faults |
| `debug_mode_mirror_is_independent` | Debug: writes don't cross |
| `debug_mode_mirror_initial_copy` | Debug: mirror starts with canonical data |
| `release_mode_mirror_shares_backing` | Release: writes cross |
| `mirror_fastmem_ineligible` | Mirror fastmem byte is 0 |
| `mirror_target_returns_canonical` | `mirror_target()` resolves |
| `mirror_target_returns_none_for_unmirrored` | `mirror_target()` returns None |
| `is_mirror_detects_mirrors` | `is_mirror()` correct for all cases |
| `unmap_canonical_with_active_mirror_fails` | BUG-1 regression test |
| `unmap_mirror_allows_canonical_unmap` | Unmap mirror → canonical unmap works |
| `unmap_mirror_cleans_up_source_tracking` | `mirror_sources` cleanup |
| `mirror_of_mirror_rejected` | BUG-3 regression test |
| `mirror_permissions_cannot_exceed_canonical` | BUG-2 regression test |

## Verification

```
cargo fmt --all -- --check         → PASS
cargo clippy --workspace --all-targets -- -D warnings → PASS
cargo test --workspace --all-targets → 215 pass, 0 fail
```

Host: `aarch64-apple-darwin`
Date: 2026-06-24
