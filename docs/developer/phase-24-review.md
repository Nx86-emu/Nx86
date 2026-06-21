# Phase 24 Review

Date: 2026-06-21

## Scope

This review covers Phase 24 from `SPEC.md`: a versioned runtime-profile format,
JIT and branch discovery logging, helper and slowmem event types, and dispatcher
integration. Profile-guided rebuild remains Phase 25.

## Findings

- `nx86-profile` writes one versioned JSON event per line and reads the same
  typed records without accepting unsupported versions or malformed complete
  lines.
- An invalid unterminated final line is treated as crash-truncated data. The
  reader reports recovery, and the writer truncates that tail before appending.
  A valid final record missing only its newline is preserved.
- Branch discovery follows the selected file-wide uniqueness policy: each
  `(source_pc, target_pc)` pair is written once, including across writer reopen.
  JIT, helper, and slowmem observations are not incorrectly deduplicated as
  branch pairs.
- The dispatcher records a published branch target before its next lookup and
  records an emergency-JIT block after compilation and cache insertion. Profile
  errors propagate as `DispatchError::Profile` and stop the run.
- Existing profile destinations must be regular files. Symlinks and directories
  are rejected, and parent directories are created for new local profile files.
- Cache names are validated against the deterministic `.nxo` key shape, and
  helper/slowmem identifiers reject path-like or free-form values.
- Records reject unknown, duplicate, empty, and oversized input. This prevents
  ignored JSON fields from carrying data outside the documented schema.
- Unix profile writers hold an exclusive file lock for their lifetime, while
  readers take a shared lock. Failed partial appends roll back to the last known
  complete record boundary.

## Boundary Checks

- Records contain guest addresses, sizes, deterministic cache names, and
  internal identifiers only. They contain no guest bytes, memory contents,
  saves, personal paths, usernames, timestamps, or host identifiers.
- This phase does not upload, share, aggregate, sanitize for export, or promote
  profile observations. Helper and slowmem events have a format and recording
  API, but their runtime producers remain later work.
- A JIT object already inserted into the cache is not rolled back if the
  subsequent profile write fails.

## Verification

Passed locally on `aarch64-apple-darwin`:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets   # 156 tests, 0 failures
cargo build --workspace
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps --document-private-items --locked
cargo check -p nx86-backend --tests --target x86_64-unknown-linux-gnu --locked
cargo audit
actionlint .github/workflows/ci.yml .github/workflows/linux-x86_64-v4.yml
shellcheck .github/scripts/*.sh
```

The Linux-only integration test is compiled on the Apple Silicon development
host but is not executed locally. It attaches a real profile writer to the
dispatcher, confirms branch then JIT record ordering, and confirms a second run
does not duplicate the branch pair.
