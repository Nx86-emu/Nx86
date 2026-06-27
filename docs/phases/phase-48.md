# Phase 48: Vulkan Backend Skeleton

Phase 48 stands up the first real graphics slice: a safe `ash` boundary that
brings up a Vulkan device, renders a frame, and surfaces it to the GUI. It is the
foundation the Phase 49–52 graphics work (shader translation, shader AOT,
pipeline cache, graphics profiles) builds on.

## What it does

- **Vulkan device setup** - `nx86-vulkan` loads the Vulkan loader at runtime,
  creates an instance, selects a physical device with a graphics queue family,
  and creates a logical device and graphics queue. All `unsafe` `ash` calls and
  raw Vulkan handles stay inside this crate.
- **Availability detection** - `VulkanAvailability::detect()` actually brings up
  a device and reports `Available` (carrying a `VulkanRenderer`) or
  `Unavailable { reason }`. No loader (the Apple Silicon dev host, headless CI)
  is a clean, non-fatal outcome.
- **Basic frame rendering** - the offscreen path renders a flat clear color into
  an `R8G8B8A8` image using core transfer/clear commands (no graphics pipeline or
  shaders yet — those arrive in Phase 49) and reads it back as tightly packed
  bytes matching `nx86_testsuite::Framebuffer`.
- **Swapchain** - a `VK_KHR_swapchain` backend (surface-capability query,
  format/present-mode selection, create/recreate, image acquisition, queue
  present) is implemented for the Linux windowed-present path.
- **GUI/runtime separation** - `nx86-gpu` orchestrates above the boundary: it
  picks the Vulkan device when present, else a deterministic CPU test-card
  renderer, and produces `RenderedFrame` RGBA8 bytes. The GUI only displays
  bytes (reusing the existing framebuffer view); the isolated runtime worker
  (`--worker runtime-smoke`) renders a frame and reports it over the versioned
  JSON-line IPC.
- **Error handling** - every Vulkan operation returns a typed `VulkanError`; the
  renderer degrades to the software frame rather than panicking, honoring the
  workspace `unwrap`/`todo` lints.

## Design

The hard host/target split shapes the API. The product target is Linux
`x86_64-v4`, but the dev host is Apple Silicon macOS with no Vulkan loader. Like
the Phase 16–18 native codegen, the logic is host-independent and the
unsupported host reports a clean unavailable outcome: the `loaded` `ash` feature
resolves the loader at runtime via `libloading`, so the Vulkan code compiles and
links everywhere and only executes where a device exists.

The verifiable, deterministic artifact behind the exit criterion is the
offscreen render-to-image → RGBA8 readback, surfaced through the existing GUI
framebuffer view. The `nx86-gpu` software fallback rasterizes a centered triangle
test card on the CPU so the fallback frame is visibly a rendered image and is
deterministic for tests on every host. On Linux with a device the GPU clear path
runs instead.

## Phase boundary

Phase 48 does not implement shader translation or a graphics pipeline (the GPU
frame is a clear color until Phase 49), nor does it wire the swapchain to a live
window — the egui GUI uses the Glow (OpenGL) renderer, so the runtime surfaces
frames as RGBA8 rather than presenting a Vulkan swapchain into the GUI window.
Live windowed presentation, descriptor/pipeline management, multi-frame
synchronization, and real title rendering remain later graphics work.
