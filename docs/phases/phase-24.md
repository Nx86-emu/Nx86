# Phase 24: Runtime Profile Logging

Phase 24 persists runtime discoveries for later profile-guided compilation.
Each title uses `profiles/runtime-v1.jsonl`; every line is an independently
versioned JSON record so complete observations remain readable if a crash leaves
the final line incomplete.

`nx86-profile` defines typed JIT-block, branch-target, helper-call, and slowmem
events. The writer repairs a truncated final line before appending, rejects
unsupported versions and malformed complete records, and refuses non-regular
destinations. A reader exposes complete records plus whether it recovered an
incomplete tail.

Branch observations are discovery data rather than hotness counters in this
phase. A `(source_pc, target_pc)` pair is written only once across the entire
profile file, including after the writer is reopened. Different sources that
reach the same target remain distinct. JIT events are recorded whenever a new
block is compiled. Helper and slowmem event APIs are available for their future
runtime paths without fabricating observations today.

The dispatcher accepts an optional `ProfileSink`. It records a branch after a
non-halting native block publishes its next guest PC and records a JIT event
after the missing block is compiled and cached. Profile failures are fatal to
that dispatch run and surface as `DispatchError::Profile`; an already cached JIT
object is not rolled back if its profile write then fails.

Profiles contain deterministic guest addresses, sizes, cache keys, and internal
reason identifiers only. Cache keys must use the exact `.nxo` key shape, while
helper and slowmem identifiers accept only a short ASCII identifier alphabet;
path-like free-form values are rejected. Phase 24 does not add upload, sharing,
title-profile sanitization UI, timestamps, host identifiers, guest bytes, memory
contents, or personal data.

## Exit Criteria

- Typed runtime records round-trip through the versioned JSONL format.
- Branch pairs remain unique across writer reopen.
- Truncated final records are recoverable without accepting malformed complete
  records.
- On Linux x86_64, a dispatcher run records its branch discovery and emergency
  JIT block, and a second run does not duplicate the branch pair.
- Profile persistence failures stop dispatch with a typed error.
