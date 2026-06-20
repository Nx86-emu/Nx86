# Phase 0-10 Review

Date: 2026-06-20

## Scope

This review covers the implemented Phase 0 through Phase 10 surface from
`SPEC.md`: project scaffolding, GUI shell, first-launch wizard, storage/title DB,
worker IPC smoke, synthetic tests, guest CPU state, narrow AArch64 decoding,
tiny interpreter execution, and the VMM skeleton.

## Findings

- Phase 0-2 foundation is in place: README, docs folders, Rust 2024 workspace,
  lint policy, CI, issue templates, egui shell, persistent settings, and the
  `nx86-vulkan` safe boundary around locked `ash` 0.38.0+1.3.281.
- Phase 3 is in place: first-launch configuration persists Linux XDG-backed
  library/cache/profile folders, fixed `x86_64-v4` CPU target, compile thread
  cap, all-core acknowledgement, profile sharing visibility, and fixed Vulkan
  backend.
- Phase 4 is in place: placeholder-only title database, deterministic title
  folders, TOML sidecars, and list/create APIs. No real title import path exists.
- Phase 5 is in place: versioned JSON-line IPC, compiler/runtime smoke workers,
  GUI progress display, and cancellation flow.
- Phase 6 is in place: TOML synthetic ARM64 tests load metadata, raw bytes,
  expected registers, and expected memory ranges.
- Phase 7 is in place: `CpuState` models GPRs, SP/PC, NZCV, FP/SIMD, FPCR/FPSR,
  thread metadata, halt state, serde debug dumps, text dumps, and expected
  register comparison.
- Phase 8 is in place: `nx86-arm64-decode` decodes the intended MOV/ADD/SUB/B/SVC
  subset with raw bytes, address, class, operands, disassembly, and errors.
- Phase 9 is in place: `nx86-runtime` executes the Phase 8 subset, updates PC and
  registers, halts on SVC, returns trace/final state, and reports expected
  register mismatches.
- Phase 10 is in place: `nx86-vmm` owns the 64 GiB arena abstraction, isolates
  Linux `mmap` unsafe code, uses a 4 KiB software page table, and reports VMM
  faults for invalid accesses.

## Boundary Checks

- No real game dump, firmware, key, save, or copyrighted binary import path was
  added.
- No JIT, AOT compiler, Phase 11 graphics, full renderer, runtime services, or
  Switch HLE behavior was added.
- `ash` is owned by `nx86-vulkan`; higher-level emulator crates do not expose raw
  Vulkan handles.
- The only workspace unsafe code is the Linux arena reservation/release block in
  `nx86-vmm`.
- `eframe` uses the native glow path for the GUI shell; `wgpu` is not used as an
  emulator graphics abstraction.

## Verification

Passed on 2026-06-20:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo build --workspace
cargo run -p nx86-app -- --worker compiler-smoke
cargo run -p nx86-app -- --worker runtime-smoke
```

Additional review commands:

```sh
cargo tree -i ash
cargo tree -i wgpu --target all
rg -n "unsafe|#\[allow\(unsafe_code\)\]" crates --glob '!target/**'
```

Manual Linux GUI acceptance remains host-dependent. This review was prepared on
a non-Linux development host, so the Linux-only GUI smoke should still be run on
the target platform.
