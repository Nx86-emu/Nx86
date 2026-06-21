# Phase 23: Emergency JIT v0

Phase 23 fills the dispatcher's missing-block seam with a compile-on-demand
fallback for the existing synthetic integer subset. The JIT uses the same
verified NxIR, register allocator, x86_64-v4 lowerer, assembler, native object
format, and cache manager as the AOT path; the difference is when one basic
block is compiled.

`nx86-x64-v4::lower_function_block` selects a block by guest entry PC and lowers
only that block while resolving branches against the complete function. An
unknown PC returns no block rather than inventing code.

`nx86-jit::EmergencyJit` owns a verified source function and `CacheManager`.
For a known missing PC it lowers the block, creates and inserts its `.nxo`
object, and emits a typed `JitEvent` plus a tracing event. The event records only
the guest PC, generated-code size, and deterministic cache file name. Persistent
profile files and profile promotion remain Phases 24-25.

An emergency-JIT-enabled `nx86-backend::Dispatcher` installs the newly compiled
bytes into executable memory and retries the same guest PC. Compilation does not
consume the native execution step budget. The event is returned in
`DispatchOutcome`; subsequent visits use the installed block without compiling
it again. Without an attached JIT, or when the source function has no block for
the PC, the existing `MissingBlock` exit remains unchanged.
The compiled block carries its own halt reason into the dispatcher, so a
function with multiple halt blocks reports the reason for the block reached.

## Exit Criteria

- A known missing basic block lowers on demand and is inserted into the `.nxo`
  cache with a typed JIT event.
- On Linux x86_64, dispatch starting with only the entry block JITs its missing
  successor, continues execution, and matches the interpreter.
- A second run reuses the installed block and emits no additional JIT event.
- An unknown guest PC remains `MissingBlock` and does not modify the cache.
- Compilation and cache behavior are unit-tested on the Apple Silicon host;
  generated-code execution remains Linux x86_64-gated.
