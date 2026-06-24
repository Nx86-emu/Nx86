# Phase 32: Memory Mirroring

Phase 32 adds memory mirroring to the VMM so multiple guest virtual addresses
can share the same backing storage. This models the Switch's DRAM aliasing
where the same physical page is accessible at multiple guest addresses.

## What it does

- **Mirror page registration** — `GuestMemory::mirror_page(mirror, canonical,
  permissions)` registers a mirror mapping. The canonical page must already be
  mapped; the mirror address must be free. Mirror pages have independent
  permissions from the canonical page.
- **Transparent address resolution** — `resolve_address()` translates mirror
  addresses to their canonical backing address before arena or logical data
  access. Permission checks use the original (mirror) address; data access uses
  the resolved (canonical) address.
- **Release mode (default)** — mirrors share backing storage. On Linux, the
  mirror offset is PROT_NONE and fastmem-ineligible; all access goes through
  the slowmem callback which resolves the redirect. In logical mode, mirrors
  have `data: None` and reads/writes resolve to the canonical page's data.
- **Debug mode** — `debug_memory_mirrors: true` in `CompilerConfig` gives each
  mirror an independent copy of the canonical page's data. Divergent writes are
  visible for debugging.
- **Fastmem eligibility** — mirror pages are always fastmem-ineligible (byte
  value 0 in the eligibility table). Canonical pages retain their fastmem
  eligibility.
- **Slowmem reason code** — the `slowmem_read` and `slowmem_write` callbacks
  emit `"mirror"` as the reason code when the faulting address is a mirror.
  The `FaultReport` match also handles the two new `VmmFault` variants.
- **Mirror introspection** — `mirror_target(address)` returns the canonical
  address for a mirror; `is_mirror(address)` checks if an address is a mirror;
  `is_debug_mode()` reports the current mode.
- **Unmap** — `unmap_mirror(mirror)` removes a mirror mapping and releases its
  page.

## Design

Mirrors are resolved via a software redirect table (`BTreeMap<u64, u64>` mapping
mirror page base to canonical page base). Mirror pages in the arena are PROT_NONE
and fastmem-ineligible, so all mirror accesses take the slowmem callback path
where the redirect is applied. This approach requires no new `unsafe` and works
within the existing arena architecture. Arena-level page sharing via
`memfd_create` is deferred to a future optimization phase.

## Phase boundary

Phase 32 owns memory mirror registration, release/debug mode semantics, and
correctness tests. It does not own: concurrent mapping mutation, SMC
invalidation, `memfd_create`-based arena-level mirroring, or mirror-aware
Native Coverage scoring.
