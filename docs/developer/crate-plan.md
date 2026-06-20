# Nx86 Crate Plan

Nx86 is a Rust monorepo. Crates are split by emulator subsystem so compiler,
runtime, GUI, storage, and graphics boundaries remain explicit from the start.

## First Prototype Target

The first prototype target is a Linux x86_64-v4 desktop application with an egui
shell. It contains a first-launch wizard, Library, Compile, Tests, and Settings
screens, persistent settings, placeholder title storage, worker IPC smoke,
synthetic ARM64 test display, guest CPU state, a narrow AArch64 decoder, a tiny
synthetic interpreter, and a VMM skeleton. It does not implement real title
import, compilation, Switch runtime execution, JIT/AOT, graphics, or rendering.

## Active Phase 0-15 Crates

- `nx86-app`: binary entrypoint, logging setup, config load, GUI launch.
- `nx86-gui`: egui shell, wizard, theme, navigation, title list, worker progress, synthetic test display, decode display, tiny interpreter status, framebuffer rendering, and NxIR dump/agreement.
- `nx86-core`: shared config, storage layout, IPC model, guest CPU state, flag (NZCV) and condition-code semantics, and Linux XDG-backed persistence.
- `nx86-title-db`: SQLite title database, deterministic title folder creation, and TOML sidecars.
- `nx86-arm64-decode`: narrow decoder for MOV/ADD/SUB/logical/loads/stores/B/B.cond/ADDS/SUBS/SVC.
- `nx86-ir`: NxIR data model (module/function/block/SSA values) and the verifier.
- `nx86-arm64-lift`: AArch64 → NxIR lifter with basic-block CFG construction.
- `nx86-ir-opt`: NxIR optimization passes (dead-flag elimination).
- `nx86-runtime`: tiny synthetic interpreter (with guest memory), NxIR evaluator, and the differential synthetic-test harness.
- `nx86-vmm`: 64 GiB guest memory arena boundary and software page-table helpers.
- `nx86-debug`: tracing-based logging setup.
- `nx86-testsuite`: synthetic ARM64 test file format, framebuffer spec, and result diffs.
- `nx86-vulkan`: internal safe boundary around `ash`.

## Skeleton Crates

The remaining crates compile as placeholders so the workspace layout matches
the specification while later phases can fill them in without reshaping the
repository. The x86_64 backend crates (`nx86-x64-asm`, `nx86-x64-v4`,
`nx86-regalloc`, `nx86-jit`, `nx86-object`) are deferred to Phases 16+, where
native code generation must be `cfg(target_arch = "x86_64")`-gated and verified
in CI because the development host is Apple Silicon.

## Vulkan Policy

Nx86 uses `ash` for the Vulkan backend because the renderer needs low-level
control over shader translation, pipeline caches, descriptor layouts,
synchronization, command buffer recording, memory management, debug tooling, and
future vendor-specific optimization.

`nx86-vulkan` owns `ash` 0.38.0+1.3.281. Cargo ignores semver build metadata in
manifest version requirements, so the workspace pins `=0.38.0` in `Cargo.toml`
and records the resolved `0.38.0+1.3.281` package in `Cargo.lock`. Raw Vulkan
handles and unsafe Vulkan calls must not leak into higher-level emulator crates.
Until Phase 48, `nx86-vulkan` exposes only safe placeholder types and Vulkan
capability metadata.

Higher-level graphics crates such as `wgpu` and `vulkano` are not core renderer
choices for Nx86 because they hide behavior that the emulator needs to control.
The Phase 2 egui shell may use eframe's native windowing/rendering path, but
that is not the emulator graphics abstraction.
