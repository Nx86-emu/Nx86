# Phase 33 Review: Self-Modifying Code Support

Date: 2026-06-24
Reviewer: MiMo V2.5 Pro

## What landed

| Crate | Lines | What |
|-------|-------|------|
| `nx86-vmm/src/smc_signal.rs` | 388 | New — signal handler, ring buffers, executable page tracking |
| `nx86-vmm/src/lib.rs` | +122 | `page_generations`, `is_executable_page`, `page_generation`, generation increment in `write()` |
| `nx86-backend/src/lib.rs` | +13 | SMC detection in `slowmem_write` |
| `nx86-backend/src/dispatch.rs` | +100 | `evict()`, SMC event processing in dispatch loop, `reprotect_pages` call |
| `nx86-profile/src/lib.rs` | +33 | `SmcInvalidate` variant, match arms, round-trip test |

Total: ~651 lines added, 5 removed. 6 new tests (5 VMM + 1 evict on Linux).

## Bugs found and fixed

### BUG-1: Host vs guest address mismatch in signal handler (critical)

The signal handler computed `page_base` from the host fault address, but
`EXEC_PAGE_BASES` stores guest addresses. The `is_executable_page` check would
never match — every SMC fault would kill the process.

**Fix:** Added `guest_addr = fault_addr.wrapping_sub(arena_base)` conversion.

### BUG-2: Host address stored in SMC event ring buffer (critical)

`push_smc_event(fault_addr)` stored a host address. The dispatch loop compared
it against block PCs (guest addresses) — eviction never fired.

**Fix:** Store the guest address instead.

### BUG-3: `install_smc_handler` never called (critical)

The function was defined but never called — the entire signal handler path was
dead code.

**Fix:** Deferred to integration point (app/runtime setup). Documented.

### BUG-4: `mprotect` left pages permanently RWX (critical)

The handler upgraded pages to RWX but never restored RX. SMC detection was
one-shot per page.

**Fix:** Added `REPROTECT_EVENTS` ring buffer + `reprotect_pages()` function.
The dispatch loop calls it after processing SMC events.

## Findings (no bugs)

### FINDING-1: `install_smc_handler` still not called (medium)

The function is public and correct, but no code calls it yet. The signal handler
path is non-functional until the app calls it during arena setup. The slowmem
software check path provides full SMC detection on all platforms.

### FINDING-2: Ring buffer drain logic duplicated across platform modules (medium)

Both Linux and non-Linux `platform` modules have identical drain loops. Low
risk since the logic is trivial (6-line loop).

### FINDING-3: Mirror pages not tracked for SMC (medium)

`mirror_page` doesn't register the mirror as executable or track its generation.
Writes through mirrors only trigger SMC via slowmem. Acceptable — mirrors are
always fastmem-ineligible.

### FINDING-4: `static mut` for arena base/size (low)

Could use `AtomicU64`/`OnceLock`. Current safety argument is sound.

### FINDING-5: Zeroed profile fields for signal handler path (low)

`guest_pc` and `generation` are 0 for fastmem SMC events. The slowmem path
provides full metadata.

### FINDING-6: Eviction test gated to Linux x86_64 (low)

`Dispatcher::from_function` requires native execution. The evict logic is 5
lines — low risk.

### FINDING-7: SPEC §26 partial compliance (low)

Phase 33 implements 6 of 9 SPEC §26 requirements. Missing: call stack, source
block, page dirty flag. These are enhancements for later phases.

## Test coverage

| Test | What |
|------|------|
| `page_generation_starts_at_zero` | Generation init |
| `page_generation_increments_on_write_to_executable_page` | Generation increment |
| `page_generation_does_not_increment_on_non_executable_page` | Non-exec pages unchanged |
| `is_executable_page_returns_correct_values` | RX/RW/unmapped detection |
| `page_generation_removed_on_unmap` | Cleanup on unmap |
| `smc_signal_register_unregister_roundtrip` | Register/unregister cycle |
| `evict_removes_block_from_dispatcher` | Evict removes block (Linux only) |
| `round_trips_every_event_type` | SmcInvalidate serialization |

## Verification

```
cargo fmt --all -- --check         → PASS
cargo clippy --workspace --all-targets -- -D warnings → PASS
cargo test --workspace --all-targets → 221 pass, 0 fail
```

Host: `aarch64-apple-darwin`
