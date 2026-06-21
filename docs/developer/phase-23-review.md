# Phase 23 Review

Date: 2026-06-21

## Scope

This review covers Phase 23 from `SPEC.md`: compiling one known missing basic
block, inserting it into both the persistent object cache and live dispatcher,
recording the event, and continuing execution.

## Findings

- `nx86-x64-v4` exposes on-demand lowering by guest entry PC without creating a
  second compiler pipeline. Full-function verification and branch-target
  resolution remain shared with AOT lowering.
- `nx86-jit::EmergencyJit` owns the verified source function and cache. It
  produces a normal `.nxo` object, inserts it before execution, and records a
  typed `JitEvent` plus a tracing event.
- `nx86-backend::Dispatcher` may attach an emergency JIT. Missing known blocks
  are installed and retried without consuming an execution step; unknown PCs
  retain the Phase 22 `MissingBlock` behavior.
- JIT events are scoped to one dispatch run. Persistent event files and
  profile-guided promotion remain Phases 24-25.

## Boundary Checks

- The JIT accepts only blocks already present in a verified NxIR function; it
  does not decode arbitrary runtime memory or discover new guest code yet.
- Conditional branches, memory operations, flags lowering, runtime profile
  files, title import, firmware, keys, HLE, graphics, and commercial software
  remain out of scope.
- Generated-code execution still crosses the existing documented unsafe ABI
  boundary. JIT-produced bytes come directly from the trusted lowerer before
  insertion into executable memory.
- Events contain deterministic compiler metadata only; they contain no paths,
  guest bytes, memory contents, or personal data.

## Verification

Passed locally on `aarch64-apple-darwin`:

```sh
actionlint .github/workflows/ci.yml .github/workflows/linux-x86_64-v4.yml
git diff --check
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets   # 134 tests, 0 failures
cargo build --workspace
```

The Linux x86_64-only integration test starts with one cached AOT block, JITs
the missing successor, continues to the expected final state, verifies the
persisted object and event, then confirms a second run emits no JIT event. It is
compiled only under `#[cfg(all(target_os = "linux", target_arch = "x86_64"))]`
and was not executed on the Apple Silicon development host.
