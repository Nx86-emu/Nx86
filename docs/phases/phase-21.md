# Phase 21: Cache Manager v0

Phase 21 turns the on-disk `.nxo` objects from Phase 20 into a managed cache.
`nx86-cache` is now filled in with a `CacheManager` over a single cache
directory, plus the supporting types `CacheEntry`, `CacheManifest`,
`CacheStatus`, and `CheckOutcome`.

`CacheManager::scan` enumerates `*.nxo` files into a `CacheManifest` (entry
address, file name, size, version) — a directory scan is always the source of
truth. `write_manifest`/`read_manifest` persist a `manifest.json` snapshot only
for fast status, so a stale manifest can never misreport what is on disk. The
manager covers integrity at two levels: a cheap `shallow_check` (the file exists
and its header magic, version, and entry address match, via the new header-only
reader) and a hash-validating `full_check` (loads the object and verifies its
FNV-1a content hash). The full check is the placeholder the SPEC §24 "full
check" upgrades later. `status`/`total_size_bytes` account for cache size, and
`insert`/`load`/`remove`/`clear` round-trip objects.

To support header-only inspection without loading and hashing each object,
`nx86-object` gained `NativeObject::read_header` + `ObjectHeader`, a shared
`object_file_name` helper (the single source of truth for the `{entry:016x}.nxo`
key), and the public `OBJECT_HEADER_LEN`.

The GUI Library screen now shows the global CPU-object cache status (object count
and total size) and a "Clear Cache" button, refreshed alongside the title list
and on service initialization. The cache directory is the storage layout's
`global_cache_dir`.

The dispatcher, emergency JIT, and runtime profile logging remain later phases.

## Exit Criteria

- The GUI shows cache status (object count and total size) for the global CPU
  object cache and can clear it.
- A compiled object inserted into the cache loads back as an identical object.
- The shallow check passes on a header-intact object whose body is corrupt,
  while the full check rejects it; both report `Missing` when no object exists.
- The cache manager is pure logic plus `std` file I/O and fully unit-tested on
  the Apple Silicon dev host.
