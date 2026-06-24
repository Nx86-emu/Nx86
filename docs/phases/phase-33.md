# Phase 33: Self-Modifying Code Support

Phase 33 adds self-modifying code detection so writes to executable guest pages
invalidate stale native blocks and trigger recompilation.

## What it does

- **Page generation tracking** — `GuestMemory` tracks a monotonic generation
  counter per page (`page_generations: BTreeMap<u64, u64>`). Initialized to 0 on
  `map_page`, incremented on every write to a page with `execute: true`, removed
  on `unmap_page`. Exposed via `page_generation(address)` and
  `is_executable_page(address)`.
- **Slowmem SMC detection** — the `slowmem_write` callback checks whether the
  written page is executable. If so, it emits a `ProfileEvent::SmcInvalidate`
  event with the guest PC, write address, page base, and current generation.
- **Signal handler (Linux)** — `nx86-vmm/src/smc_signal.rs` installs a `SIGSEGV`
  handler via `sigaction`. On Linux, executable pages are mapped `RX` at the host
  level (no write). A fastmem write triggers SIGSEGV, the handler converts the
  host fault address to a guest address, upgrades the page to `RWX` via
  `mprotect`, and pushes the event to an async-signal-safe ring buffer. A second
  ring buffer records pages that need re-protection back to `RX`.
- **Re-protection** — `reprotect_pages()` drains the reprotect ring buffer and
  calls `mprotect(PROT_READ | PROT_EXEC)` on each page, restoring the `RX`
  protection so future writes trigger SMC detection again.
- **Dispatcher eviction** — `Dispatcher::evict(pc)` invalidates chains and
  removes the compiled block. The dispatch loop drains both slowmem and signal
  handler SMC events after each block execution, evicts all blocks on affected
  pages, and re-protects pages.
- **Profile event** — `ProfileEvent::SmcInvalidate` serializes to JSON with
  fields: `guest_pc`, `write_address`, `page_base`, `generation`.

## Design

SMC detection has two paths:

1. **Slowmem path** (all platforms): writes that go through the `slowmem_write`
   callback are checked against `is_executable_page`. This covers logical-mode
   writes, mirror writes, and any write that fails the fastmem check.

2. **Fastmem path** (Linux only): executable pages are mapped `RX` at the host
   level. Fastmem writes to `RX` pages trigger SIGSEGV. The signal handler
   catches the fault, upgrades to `RWX`, and records the event. After the
   dispatch loop processes the event, `reprotect_pages()` restores `RX`.

On non-Linux platforms (macOS dev host), the signal handler module provides
no-op stubs and all SMC detection goes through the slowmem path.

The ring buffers are SPSC (single-producer, single-consumer) with atomic
Acquire/Release ordering. The signal handler is the producer (runs on the
faulting thread, preempts synchronously), and the dispatch loop is the consumer.

## Phase boundary

Phase 33 owns page generation tracking, slowmem SMC detection, the signal
handler, dispatcher eviction, and `SmcInvalidate` profile events. It does not
own: concurrent mapping mutation, mirror SMC tracking, `install_smc_handler`
integration (app-level concern), call stack/source block metadata in SMC events,
or page dirty flags.
