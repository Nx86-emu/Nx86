# Phase 30: Fastmem v0

Phase 30 turns NxIR `Load` and `Store` operations into native x86-64 memory
operations. A generated block checks a byte-per-page eligibility table and,
when the access is eligible, addresses guest RAM as `fastmem_base + guest_address`.
The actual load or store is then one native instruction.

## What it does

- **Arena-backed guest RAM** — on Linux, `GuestMemory::new` reserves the 64 GiB
  arena and commits mapped pages in place. The checked VMM APIs and generated
  code therefore observe the same bytes. Mapping resets a page, applies host
  protections, and publishes read/write eligibility; unmapping revokes access,
  discards the page, and clears eligibility. `new_logical` retains its portable
  software backing and deliberately has no fastmem view.
- **Memory base ABI** — native blocks preserve `r12`-`r15`. `r15` holds the
  `NativeBlockState`, `r14` the arena base, `r13` the permission table, and
  `r12` the slowmem context. Spill slots begin below those saved registers, and
  both ordinary returns and Phase 29 chain exits restore the complete frame.
- **Direct loads and stores** — I32/I64 accesses check the arena bound, ensure
  the operation stays within one 4 KiB page, and test the required read/write
  bit before issuing an indexed native load or store. I32 loads zero-extend;
  I32 stores write only the low word. `Trunc` and `ZeroExtend` now lower as the
  conversion operations emitted around AArch64 word accesses.
- **Checked fallback** — an ineligible access calls a narrow Rust ABI helper.
  The helper uses the existing VMM `read`/`write` implementation, so logical
  memory, cross-page accesses, protected pages, unmapped pages, and out-of-range
  addresses remain checked. Successful helpers resume the native block. A fault
  records the guest instruction PC, restores the native frame, and returns a
  typed `NativeMemoryError` instead of continuing or causing a host fault.
- **Differential execution** — tiny-block and dispatcher APIs accept a mutable
  `GuestMemory`. Synthetic tests build independent native memory, then compare
  both final CPU state and observable memory with the AArch64 interpreter.

## Phase boundary

The executable helper seam is included now so fastmem always has a safe
fallback. Phase 31 still owns slowmem counters, profile reporting, detailed
crash reports, UI pause behavior, and the Native Coverage penalty. Phase 32
owns mirroring. Concurrent mapping mutation and SMC invalidation remain later
work.

## Exit criteria

The synthetic word/doubleword memory program lowers through the native path.
On Linux x86_64, an integration test executes it once against arena-backed
fastmem and once against logical memory (forcing the helper path), verifies the
final register state and stored bytes, and confirms an unmapped access becomes
a typed error.
