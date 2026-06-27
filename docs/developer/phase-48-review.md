# Phase 48 Review: Vulkan Backend Skeleton

Date: 2026-06-27
Reviewer: Claude (Opus 4.8)

## What landed

| Crate | What |
|-------|------|
| `nx86-vulkan/src/context.rs` | loader-based `Entry::load`, instance, physical-device + graphics-queue selection, logical device (best-effort `VK_KHR_swapchain`), memory-type finder, RAII `Drop` |
| `nx86-vulkan/src/offscreen.rs` | offscreen render-to-image: clear → image-to-buffer copy → `R8G8B8A8` readback, fully fenced |
| `nx86-vulkan/src/swapchain.rs` | `VK_KHR_swapchain` backend: caps/format/present-mode selection, create/recreate, acquire, present (crate-internal, Linux windowing path) |
| `nx86-vulkan/src/error.rs` | typed `VulkanError` for loader/instance/device/surface/swapchain/allocation/command/present |
| `nx86-vulkan/src/lib.rs` | `VulkanAvailability::detect`, `VulkanRenderer`, retained `VulkanBackendInfo`; `ash` `loaded` feature |
| `nx86-gpu/src/lib.rs` | `Renderer` (Vulkan-or-software), `RenderedFrame`, CPU test-card rasterizer, `Backend` label |
| `nx86-gui` | "Render demo frame" control + backend label, framebuffer view moved out of the loaded-test block |
| `nx86-app/src/main.rs` | `runtime-smoke` worker renders a frame and reports it over JSON-line IPC |

## Findings

### FINDING-1: host has no Vulkan loader, so device work can't run on the dev host

The Apple Silicon dev host has no Vulkan loader (`dlopen(libvulkan.dylib)` fails),
matching the Phase 16–18 native-codegen precedent. The `ash` `loaded` feature
resolves the loader at runtime via `libloading`, so all Vulkan code compiles and
links on the dev host and in CI but executes only where a device exists.
`VulkanAvailability::detect()` returns `Unavailable { reason }` there; callers
fall back to the deterministic software frame. Runtime correctness of the GPU
offscreen and swapchain paths is verified on Linux `x86_64-v4`, not on the dev
host.

### FINDING-2: the verifiable, deterministic frame is the CPU test card (fixed)

To keep the exit criterion testable on every host, `nx86-gpu` renders a
CPU-rasterized triangle test card when Vulkan is unavailable, with exact-pixel
assertions. The Vulkan offscreen path renders a flat clear color (no shaders
until Phase 49) and asserts the cleared color round-trips through `R8G8B8A8` when
a device is present.

### FINDING-3: the boundary rule is preserved (fixed)

No raw `ash`/Vulkan handles leak past `nx86-vulkan`: `nx86-gpu`, the GUI, and the
runtime see only RGBA8 bytes (the shared `nx86_testsuite::Framebuffer`) and
string labels. The swapchain backend, whose API necessarily deals in `vk::`
handles, is kept crate-internal (`#![allow(dead_code)]`) until the Linux
windowing layer drives it.

### FINDING-4: swapchain is not wired to a live window this phase (boundary)

The egui GUI uses the Glow (OpenGL) renderer, so a Vulkan swapchain cannot
present into the GUI window. The swapchain backend is implemented and compiled;
the runtime surfaces frames as RGBA8 instead. Live windowed presentation is
deferred.

## Post-review hardening

A recall-biased multi-angle code review of this phase found and fixed the
following before sign-off:

- **Offscreen resource leak (high):** `offscreen::render_clear` freed its image,
  buffers, memory, and command pool only on the success path; any `?` error
  leaked them. Resources now live in a `FrameResources` RAII guard freed on every
  path (`Drop`), and `bind_*_memory` frees the allocation if `bind` fails.
- **Swapchain-on-fallback-device (medium):** `create_device` could silently fall
  back to a device without `VK_KHR_swapchain`; `Swapchain::new` would then call
  swapchain commands on it (UB). The context records `swapchain_enabled` and the
  swapchain path now returns `VulkanError::Swapchain` cleanly when it is unset.
- **Device teardown ordering (medium):** `VulkanContext::Drop` now calls
  `device_wait_idle` before `destroy_device` so any in-flight (e.g. async present)
  work completes first.
- **Swapchain leak (medium):** `Swapchain` now frees its handle in `Drop` instead
  of only via an explicit `destroy()`, so scope-exit/`?`/panic cannot leak it.
- **Stale UI label (low):** the demo-render backend label is cleared alongside the
  framebuffer when a synthetic test is loaded/analyzed.
- **Reuse + per-click cost (low):** `nx86-gpu` reuses the shared
  `nx86_testsuite::Framebuffer` instead of a duplicate `RenderedFrame`, and the
  GUI caches one `Renderer` instead of re-detecting Vulkan on every click.

## Test coverage

| Test | What |
|------|------|
| `detect_never_panics_and_reports_a_reason_when_unavailable` | availability seam, both branches |
| `offscreen_clear_matches_requested_color_when_a_device_exists` | GPU clear → RGBA8 round-trip (device-gated) |
| `test_card_paints_background_corners_and_foreground_center` | deterministic CPU test card |
| `test_card_is_deterministic` | identical output across runs |
| `renderer_always_produces_a_frame_of_requested_size` | renderer yields a frame on any host |
| `render_demo_frame_populates_framebuffer_and_backend` | GUI wiring into the framebuffer view |
| `analyzing_a_test_clears_a_stale_renderer_backend_label` | stale-label regression (post-review) |

## Verification

Host-independent (Apple Silicon dev host):

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo run -p nx86-app -- --worker runtime-smoke   # emits a "rendered 16x12 frame via ..." log event
```

All passed. On the dev host the runtime-smoke worker reports the software
fallback; on Linux `x86_64-v4` it reports `Vulkan (<device>)` and the GUI
framebuffer view displays the rendered frame, satisfying the exit criterion
("runtime displays a rendered frame").
