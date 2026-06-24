# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Nx86 is a GUI-first Nintendo Switch emulator and AArch64→x86_64-v4 native binary
translation system, written as a Rust monorepo. Its defining technique is
**Continuous Dynamic Compilation (CDC)**: compile a title before play, profile
runtime fallbacks, promote discoveries, and passively rebuild toward higher
**Native Coverage** (the project's main progress metric — functional readiness
toward "Pure AOT" native output, not bytes compiled).

Nx86 includes **NDX (N Dynamic X)**, a built-in modloader tightly integrated
with the AOT-first runtime. NDX is core architecture, not an optional plugin
system. See the NDX section in `SPEC.md` for the full spec.

`SPEC.md` is authoritative for direction, terminology, roadmap, platform
strategy, and the legal boundary. When code and SPEC.md disagree, SPEC.md wins.
Read it before making non-trivial design decisions.

## Build, run, and checks

```sh
cargo build --workspace
cargo run -p nx86-app                       # egui desktop shell (Linux only)
cargo run -p nx86-app -- --worker compiler-smoke   # JSON-line worker IPC smoke
cargo run -p nx86-app -- --worker runtime-smoke
```

The full check set (matches CI in `.github/workflows/ci.yml`, all must pass):

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
```

Run a single test or crate:

```sh
cargo test -p nx86-runtime                  # one crate
cargo test -p nx86-runtime run_synthetic    # tests matching a substring
```

## Critical host/target constraint

The development host is **Apple Silicon (`aarch64-apple-darwin`)**, but the
product target is **Linux `x86_64-v4`**. This shapes everything:

- Phases 0–15 are deliberately **host-independent** — pure logic, no native
  code generation — so they run and test on the Mac dev host. Phases 16–18 add
  the first native-codegen slice, but actual code execution is available only on
  Linux x86_64; Apple Silicon reports a clean unsupported-host outcome.
- The GUI shell (`cargo run -p nx86-app`) is **Linux-only**; do not expect it to
  run on the dev host. Exercise GUI changes on the Linux target.
- The Phase 16–29 backend crates (`nx86-x64-asm`, `nx86-jit`, `nx86-x64-v4`,
  `nx86-backend`, `nx86-regalloc`, `nx86-object`, `nx86-cache`) are active for
  the narrow integer path: register allocation with spills, persistent `.nxo`
  objects, a managed cache, multi-block dispatch through unconditional branches,
  emergency single-block JIT fallback, guarded forward block chaining, runtime
  profiles, profile-guided rebuilds, CFG inspection, and guard/deopt metadata.

## Architecture

The repo is split into ~36 crates by emulator subsystem so compiler, runtime,
GUI, storage, and graphics boundaries stay explicit. Many are placeholders that
compile but await later phases (see `docs/developer/crate-plan.md` for the
active-vs-skeleton breakdown). The currently substantive pieces:

**Compiler front half (the working pipeline today):**
`nx86-arm64-decode` (narrow AArch64 decoder: MOV/ADD/SUB/logical/loads/stores/
B/B.cond/ADDS/SUBS/CMP/SVC) → `nx86-arm64-lift` (AArch64 → NxIR with basic-block
CFG) → `nx86-ir` (**NxIR**: the shared SSA IR used by both AOT and JIT — module/
function/block/typed-SSA-values + a verifier in `verify.rs`) → `nx86-ir-opt`
(passes, e.g. dead-flag elimination).

**Differential testing oracle:** `nx86-runtime` contains both a tiny synthetic
interpreter and an independent NxIR evaluator (`eval.rs`). `run_synthetic_test`
runs a program through both and asserts they agree on final register state and
observed memory. It also attempts the Phase 18 native x86_64 tiny-block path
from the same verified NxIR function. On Apple Silicon, native execution should
report `unavailable`; on Linux x86_64, supported single- and multi-block
programs should match the interpreter.

**Native backend slice:** `nx86-x64-asm` owns exact byte emission for the small
assembler API; `nx86-jit` owns executable memory, trusted generated-code call
wrappers, and the emergency compiler/cache path; `nx86-x64-v4` lowers the
supported NxIR integer subset per block;
`nx86-object` and `nx86-cache` persist and manage native blocks; `nx86-backend`
orchestrates single-block execution and multi-block dispatch.

**Shared foundations:**
- `nx86-core`: shared config, storage layout, the IPC model (`ipc.rs`), and
  guest CPU state + NZCV flag / condition-code semantics (`guest.rs`). Flag and
  condition semantics live **once** here — do not duplicate them in the lifter.
- `nx86-vmm`: 64 GiB guest-memory arena boundary + software page tables. Its
  Linux arena reservation is one of the only allowed scoped `unsafe` islands.
- `nx86-jit`: executable-memory allocation and generated-code calls. Keep all
  mmap/transmute `unsafe` inside its platform module with narrow safety comments
  and W^X permissions. Executing generated code is an explicit `unsafe` API
  contract; backend call sites must document why the generated bytes match the
  requested ABI.
- `nx86-title-db`: SQLite title database + human-readable TOML sidecars.
- `nx86-testsuite`: synthetic ARM64 test file format and framebuffer spec.
- `nx86-vulkan`: the safe boundary around `ash`. Raw Vulkan handles and unsafe
  Vulkan calls **must not leak** into higher-level crates; until Phase 48 it
  exposes only safe placeholders + capability metadata. `ash`/`wgpu`/`vulkano`
  are not interchangeable here — see the Vulkan policy in `crate-plan.md`.

**App + GUI:** `nx86-app` is the thin binary entrypoint (logging, config load,
GUI launch, worker dispatch). `nx86-gui` is the egui shell (wizard, Library/
Compile/Tests/Settings screens, framebuffer rendering, NxIR and native dump
views).

**NDX modloader:** `nx86-ndx` is the built-in modloader crate. It handles mod
discovery (`scanner.rs`), safe archive import (`archive_import.rs`), `nxmod.toml`
metadata (`manifest.rs`), validation and conflict detection (`validator.rs`),
virtual RomFS overlays (`overlay.rs`), extension layers (`romfs_ext.rs`), code
patch operations (`code_patch.rs`), runtime cheats (`cheat.rs`), TOML-backed
per-game profiles (`profile.rs`), cache impact planning (`cache_plan.rs`),
repository and GitHub API integration (`repository.rs`), native/Lua trust
decisions (`trust.rs`), and GUI state exposure (`ui_model.rs`). The native mod
format is `.ndx` with required `nxmod.toml` metadata. NDX also supports
Eden-compatible mod imports (`romfs/`, `romfs_ext/`, `exefs/`, `cheats/`).
Mods are stored per-game inside the compiled/cache folder by Title ID.
Drag-and-drop is the primary import UX. NDX integrates with the AOT compiler's
cache planning — partial recompilation is preferred, stale cache is hard-blocked.

**Process model:** multi-process by design — GUI process, isolated compiler
worker process, and isolated per-title runtime process, talking over a
**versioned JSON-line IPC** layer. The GUI never executes guest code; only one
title runs at a time. Worker modes already emit versioned JSON-line events.

## Conventions

- Workspace lints (in root `Cargo.toml`) are warnings the project treats as
  hard: `unsafe_code`, `unwrap_used`, `todo`, `dbg_macro`,
  `rust_2018_idioms`. Clippy runs with `-D warnings` in CI, so any of these
  fails the build. Avoid `.unwrap()`, `todo!()`, `dbg!()`, and new `unsafe`.
- Edition 2024, `rustfmt.toml` sets `max_width = 100` and Unix newlines.
- Phases are tracked in `docs/phases/`; each completed range gets a review in
  `docs/developer/` (e.g. `phase-11-15-review.md`) that records what landed,
  boundary checks, and the verification run. Follow that pattern.

## Legal boundary (hard rule)

Nx86 must not ship or request copyrighted game dumps, proprietary firmware,
console keys, proprietary SDK code, shared saves, copyrighted binary blobs, or
personal user data. The prototype does not import or run real Switch software;
the first real software target after the synthetic native-codegen foundations is
homebrew. Do not add code paths that import or embed any of the above.

NDX repository/index features must never download, distribute, or assist with
obtaining Nintendo keys, title keys, firmware, copyrighted game files,
copyrighted Nintendo system files, or leaked SDK content. Users are responsible
for using mods legally with their own legally obtained game content.
