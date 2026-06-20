# Phase 10: VMM Skeleton

Phase 10 adds the VMM skeleton in `nx86-vmm`. Linux builds reserve a 64 GiB
guest arena with `mmap(PROT_NONE | MAP_NORESERVE)` inside the VMM crate only;
other platforms use a logical fallback for tests. Guest memory uses a 4 KiB
software page table with map, unmap, read, write, permission, bounds, and debug
dump helpers.

## Exit Criteria

- Synthetic memory ranges can be mapped, written, read, and dumped.
- VMM faults report unmapped, permission, out-of-range, and cross-page invalid
  access errors.
