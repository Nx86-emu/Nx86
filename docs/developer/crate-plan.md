# Nx86 Crate Plan

Nx86 is a Rust monorepo. Crates are split by emulator subsystem so compiler,
runtime, GUI, storage, and graphics boundaries remain explicit from the start.

## First Prototype Target

The first prototype target is a Linux x86_64-v4 desktop application with an egui
shell. It contains a first-launch wizard, Library, Compile, Tests, and Settings
screens, persistent settings, placeholder title storage, worker IPC smoke,
synthetic ARM64 test display, guest CPU state, a narrow AArch64 decoder, a tiny
synthetic interpreter, an NxIR differential oracle, a VMM skeleton, a simple
homebrew descriptor loader, an input runtime, a guest IPC v0 model, an audio
runtime skeleton, a skeleton HLE service dispatcher, and the first internal
x86_64 native-codegen path for
synthetic programs. The native path includes register allocation and spills,
persistent `.nxo` objects, a managed cache, multi-block dispatch,
compile-on-demand fallback, versioned runtime profiles, native fastmem with
checked VMM fallback, basic threading/replay structures, FP/SIMD coverage,
hot/cold layout, and Native Coverage reporting. It does not implement commercial
title import, real NRO/NSO containers, complete Horizon services, graphics,
motion/rumble, or Switch rendering.

## Active Phase 0-47 Crates

- `nx86-app`: binary entrypoint, logging setup, config load, GUI launch.
- `nx86-gui`: egui shell, wizard, theme, navigation, title list, worker progress, synthetic test display, decode display, tiny interpreter status, framebuffer rendering, NxIR/native dump agreement, Inspector views, input settings/status, and audio backend status.
- `nx86-core`: shared config, storage layout, IPC model, guest CPU state, flag (NZCV) and condition-code semantics, Linux XDG-backed persistence, input config, and the experimental native-patch-chaining preference.
- `nx86-title-db`: SQLite title database, deterministic title folder creation, TOML sidecars, synthetic-program title content for the Inspector, and homebrew module content persistence.
- `nx86-import`: simple `.nxhb.toml` homebrew descriptor parsing, validation, and guest-memory mapping.
- `nx86-arm64-decode`: narrow decoder for MOV/ADD/SUB/logical/loads/stores/B/B.cond/ADDS/SUBS/SVC.
- `nx86-ir`: NxIR data model (module/function/block/SSA values), the verifier,
  and speculation primitives — a guard terminator with a per-function deopt-point
  table (Phase 28).
- `nx86-arm64-lift`: AArch64 → NxIR lifter with basic-block CFG construction,
  plus recursive-descent CFG recovery (`recover`) that derives blocks and
  function candidates from an entry PC for the Inspector.
- `nx86-ir-opt`: NxIR optimization passes (dead-flag elimination).
- `nx86-inspector`: host-independent Inspector composition — from a guest
  program's bytes and entry PC it produces disassembly, the recovered CFG, the
  lifted NxIR dump, and the native (x86_64) mapping, degrading gracefully when
  lifting or lowering is unavailable.
- `nx86-runtime`: tiny synthetic interpreter (with guest memory), NxIR evaluator (with guard/deopt routing via `EvalOutcome`), native backend attempt, CPU-plus-memory differential synthetic-test harness, homebrew boot helper with skeleton service continuation, injectable input snapshots/providers, and guest IPC audio dispatch.
- `nx86-backend`: native execution orchestration, interpreter comparison, memory attachment with typed slowmem fallback, and the multi-block dispatcher with emergency-JIT fallback, software chain caching, guarded native chain installation, statistics, and invalidation.
- `nx86-x64-asm`: internal x86_64 assembler API, labels, indexed and width-specific memory operands, code buffer, dump, control-flow/call primitives, and fixed-size runtime patch sites.
- `nx86-jit`: Linux x86_64 executable memory, trusted generated-code call wrappers, W^X runtime patching, and on-demand single-block compilation into the object cache.
- `nx86-x64-v4`: single-block and per-block NxIR integer/memory lowering to x86_64 bytes, including inline fastmem checks, slowmem callback exits, unconditional-branch routing, and chain-exit metadata for the dispatcher.
- `nx86-regalloc`: basic linear-scan register allocator for a single NxIR block (pool registers with stack-slot spills).
- `nx86-object`: AOT object format v0 — `.nxo` serialization of a native block with guest mapping and a validation hash.
- `nx86-cache`: cache manager v0 — `.nxo` object directory with a manifest, shallow/full integrity checks, size accounting, and insert/load/remove/clear.
- `nx86-profile`: strict versioned JSONL runtime profiles with typed events,
  crash-tail recovery, file-wide branch-pair deduplication, Unix file locking,
  destination hardening, and partial-append rollback.
- `nx86-vmm`: 64 GiB guest memory arena, page-backed Linux fastmem, eligibility metadata, checked software page-table helpers, and page permission transitions for loader seeding.
- `nx86-hle`: skeleton service dispatcher for homebrew exit plus filesystem, thread, memory, input, and audio service stubs.
- `nx86-service`: guest IPC v0 command/response buffers, sessions, domains, object handles, descriptors, handle transfers, and result codes.
- `nx86-input`: controller button bitset, input snapshots, and default-feature `gilrs` gamepad polling for the Phase 46 input runtime.
- `nx86-audio`: default-feature `cpal` host audio backend, null sink fallback, stereo `f32` buffer queue, frame counters, and deterministic timing hooks.
- `nx86-debug`: tracing-based logging setup.
- `nx86-testsuite`: synthetic ARM64 test file format, framebuffer spec, and result diffs.
- `nx86-vulkan`: internal safe boundary around `ash` — loader-based availability
  detection, instance/physical-device/logical-device bring-up, an offscreen
  render-to-image (clear → `R8G8B8A8` readback) frame path, and a
  windowed-present swapchain backend. All `unsafe` `ash` and raw Vulkan handles
  stay inside this crate.
- `nx86-gpu`: renderer orchestration above `nx86-vulkan` — selects the Vulkan
  device when present, else a deterministic CPU test-card fallback, and yields
  the shared `nx86_testsuite::Framebuffer` RGBA8 bytes for the GUI framebuffer
  view.
- `nx86-shader`: shader translation skeleton — `ShaderStage`/`ShaderMetadata`
  model, FNV-1a `ShaderHash`, a deterministic placeholder `translate` path, the
  integrity-checked `.nxshader` object container, and a `ShaderCache` (mirroring
  `nx86-cache`) over a title's `cache/shaders/` folder. No real SPIR-V/GPU yet.

## Skeleton Crates

The remaining crates compile as placeholders so the workspace layout matches
the specification while later phases can fill them in without reshaping the
repository. Profile-guided rebuild and broader runtime/graphics crates remain
deferred to Phases 25+.

## Vulkan Policy

Nx86 uses `ash` for the Vulkan backend because the renderer needs low-level
control over shader translation, pipeline caches, descriptor layouts,
synchronization, command buffer recording, memory management, debug tooling, and
future vendor-specific optimization.

`nx86-vulkan` owns `ash` 0.38.0+1.3.281. Cargo ignores semver build metadata in
manifest version requirements, so the workspace pins `=0.38.0` in `Cargo.toml`
and records the resolved `0.38.0+1.3.281` package in `Cargo.lock`. Raw Vulkan
handles and unsafe Vulkan calls must not leak into higher-level emulator crates.
As of Phase 48, `nx86-vulkan` is a real safe boundary: it loads the Vulkan loader
at runtime (the `loaded` feature), so hosts without a loader — the Apple Silicon
dev host and headless CI — report a clean `Unavailable` and callers fall back to
a deterministic software frame instead of failing to build, link, or run.

Higher-level graphics crates such as `wgpu` and `vulkano` are not core renderer
choices for Nx86 because they hide behavior that the emulator needs to control.
The Phase 2 egui shell may use eframe's native windowing/rendering path, but
that is not the emulator graphics abstraction.
