# Phase 24: Runtime Profile Logging

Phase 24 persists runtime discoveries for later profile-guided compilation.
Each title uses `profiles/runtime-v1.jsonl`; every line is an independently
versioned JSON record so complete observations remain readable if a crash leaves
the final line incomplete.

`nx86-profile` defines typed JIT-block, branch-target, helper-call, and slowmem
events. The writer repairs a truncated final line before appending, rejects
unsupported versions, oversized or structurally unexpected records, and
malformed complete records. It also refuses non-regular destinations. A reader
exposes complete records plus whether it recovered an incomplete tail.

## Wire Format

The format is strict JSONL. Every record has `format_version: 1`, one `kind`,
and exactly the fields for that kind:

```json
{"format_version":1,"kind":"jit_block","guest_pc":4096,"code_size_bytes":42,"cache_file_name":"0000000000001000.nxo"}
{"format_version":1,"kind":"branch_target","source_pc":4096,"target_pc":8192}
{"format_version":1,"kind":"helper_call","guest_pc":8192,"helper_id":"svc.dispatch"}
{"format_version":1,"kind":"slowmem","guest_pc":8196,"address":32768,"size_bytes":8,"access":"read","reason_code":"page-not-fastmem"}
```

Record order is observation order; JSON object field order is not significant.
Unknown, missing, or duplicate fields, empty lines, unsupported versions, and
records larger than 16 KiB (excluding the newline) are rejected. JIT code size
must be nonzero. Slowmem size must be 1, 2, 4, 8, or 16 bytes, and its access is
`read`, `write`, or `execute`. Cache names are exactly 16 lowercase hexadecimal
digits plus `.nxo`. Helper and reason identifiers are 1-64 ASCII letters,
digits, dots, hyphens, or underscores.

Only a final line whose JSON syntax is incomplete is recoverable crash-tail
data. A syntactically complete record without a final newline is valid and the
writer adds the missing newline before appending. A complete record that fails
schema, version, or field validation is an error even when it has no newline.

Branch observations are discovery data rather than hotness counters in this
phase. A `(source_pc, target_pc)` pair is written only once across the entire
profile file, including after the writer is reopened. On Unix targets, an
exclusive lifetime lock prevents concurrent writers from violating this rule;
readers use a shared, nonblocking lock and consume the file after the writer
closes. A second writer or a reader opened while a writer owns the file returns
`ProfileError::AlreadyLocked` rather than observing a partial append.
Different sources that reach the same target remain distinct. JIT events are
recorded whenever a new block is compiled. Helper and slowmem event APIs are
available for their future runtime paths without fabricating observations
today.

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

Each append tracks the last complete byte boundary. If a write fails after a
partial append, the writer truncates back to that boundary before returning the
fatal error; a rollback failure is reported separately.

## Exit Criteria

- Typed runtime records round-trip through the versioned JSONL format.
- Branch pairs remain unique across writer reopen.
- Truncated final records are recoverable without accepting malformed complete
  records.
- On Linux x86_64, a dispatcher run records its branch discovery and emergency
  JIT block, and a second run does not duplicate the branch pair.
- Profile persistence failures stop dispatch with a typed error.
