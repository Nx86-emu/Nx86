//! Safe boundary around `ash` for the Nx86 renderer.
//!
//! Raw Vulkan handles and unsafe `ash` calls are confined to this crate; higher
//! crates (`nx86-gpu`, the runtime, the GUI) see only safe types. The hard
//! host/target split (Apple Silicon dev host vs. Linux `x86_64-v4` product)
//! shapes the API: [`VulkanAvailability::detect`] loads the Vulkan loader and
//! reports a clean [`VulkanAvailability::Unavailable`] when none is present, so
//! callers can fall back to a deterministic software frame instead of failing.
//!
//! Phase 48 implements device setup ([`context`]), an offscreen render-to-image
//! frame path ([`offscreen`]) that produces `R8G8B8A8` bytes for the existing
//! GUI framebuffer view, and a windowed-present swapchain backend
//! ([`swapchain`]). The offscreen path is the verifiable artifact behind the
//! exit criterion; the swapchain executes on Linux with a real surface.

mod context;
mod error;
mod offscreen;
pub mod pipeline_cache;
mod swapchain;

pub use context::VulkanContext;
pub use error::{VulkanError, VulkanResult};

/// Capability metadata about the Vulkan binding, independent of whether a device
/// is actually present. Retained from the pre-Phase-48 placeholder.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VulkanBackendInfo {
    binding: &'static str,
    api_version: u32,
}

impl VulkanBackendInfo {
    #[must_use]
    pub fn current() -> Self {
        Self {
            binding: "ash",
            api_version: ash::vk::API_VERSION_1_3,
        }
    }

    #[must_use]
    pub const fn binding(self) -> &'static str {
        self.binding
    }

    #[must_use]
    pub const fn api_version(self) -> u32 {
        self.api_version
    }
}

impl Default for VulkanBackendInfo {
    fn default() -> Self {
        Self::current()
    }
}

/// Whether a usable Vulkan device is present on this host.
///
/// Detection actually loads the loader and brings up a device, so an
/// [`Available`](Self::Available) result means rendering can proceed.
pub enum VulkanAvailability {
    /// A device is ready; carries the initialized renderer. Boxed because a
    /// ready renderer is far larger than the `Unavailable` reason string.
    Available(Box<VulkanRenderer>),
    /// No usable Vulkan device; carries a human-readable reason. Expected on the
    /// Apple Silicon dev host and in headless CI.
    Unavailable { reason: String },
}

impl VulkanAvailability {
    /// Attempt to bring up Vulkan, never panicking. Loader/device failures are
    /// reported as [`Unavailable`](Self::Unavailable) with a reason.
    #[must_use]
    pub fn detect() -> Self {
        match VulkanRenderer::new() {
            Ok(renderer) => Self::Available(Box::new(renderer)),
            Err(err) => Self::Unavailable {
                reason: err.to_string(),
            },
        }
    }

    /// `true` when a renderer is available.
    #[must_use]
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available(_))
    }
}

/// A ready-to-use Vulkan renderer owning an initialized device.
///
/// The renderer hands back `R8G8B8A8` frames; it never exposes raw `ash`
/// handles. Frame contents match `nx86_testsuite::Framebuffer` byte layout.
pub struct VulkanRenderer {
    context: VulkanContext,
}

impl core::fmt::Debug for VulkanRenderer {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("VulkanRenderer")
            .field("device_name", &self.context.device_name())
            .finish()
    }
}

impl core::fmt::Debug for VulkanAvailability {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Available(renderer) => f.debug_tuple("Available").field(renderer).finish(),
            Self::Unavailable { reason } => f
                .debug_struct("Unavailable")
                .field("reason", reason)
                .finish(),
        }
    }
}

impl VulkanRenderer {
    /// Bring up a Vulkan device. Returns [`VulkanError::LoaderUnavailable`] when
    /// no loader is installed.
    pub fn new() -> VulkanResult<Self> {
        Ok(Self {
            context: VulkanContext::new()?,
        })
    }

    /// Name of the physical device backing this renderer.
    #[must_use]
    pub fn device_name(&self) -> &str {
        self.context.device_name()
    }

    /// Borrow the underlying context (for the swapchain windowing layer).
    #[must_use]
    pub fn context(&self) -> &VulkanContext {
        &self.context
    }

    /// Render a flat clear color into a `width`×`height` offscreen target and
    /// return tightly packed `R8G8B8A8` bytes (`width * height * 4` long).
    pub fn render_clear(&self, width: u32, height: u32, color: [f32; 4]) -> VulkanResult<Vec<u8>> {
        offscreen::render_clear(&self.context, width, height, color)
    }
}

/// Capability metadata accessor (binding + API version).
#[must_use]
pub fn backend_info() -> VulkanBackendInfo {
    VulkanBackendInfo::current()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_info_reports_ash_binding() {
        let info = backend_info();
        assert_eq!(info.binding(), "ash");
        assert_eq!(info.api_version(), ash::vk::API_VERSION_1_3);
    }

    #[test]
    fn detect_never_panics_and_reports_a_reason_when_unavailable() {
        // On the Apple Silicon dev host / headless CI this takes the Unavailable
        // branch with a non-empty reason; on Linux with a device it takes the
        // Available branch. Either outcome is a clean, panic-free result.
        match VulkanAvailability::detect() {
            VulkanAvailability::Available(renderer) => {
                assert!(!renderer.device_name().is_empty());
            }
            VulkanAvailability::Unavailable { reason } => {
                assert!(!reason.is_empty(), "unavailable reason must explain why");
            }
        }
    }

    #[test]
    fn offscreen_clear_matches_requested_color_when_a_device_exists() {
        // Deterministic on any host: skip cleanly when no device, otherwise the
        // clear color must round-trip through the offscreen image to R8G8B8A8.
        let VulkanAvailability::Available(renderer) = VulkanAvailability::detect() else {
            return;
        };
        let pixels = renderer
            .render_clear(2, 2, [1.0, 0.0, 0.0, 1.0])
            .expect("offscreen render should succeed on an available device");
        assert_eq!(pixels.len(), 2 * 2 * 4);
        // R8G8B8A8_UNORM: first channel is red.
        for pixel in pixels.chunks_exact(4) {
            assert_eq!(pixel, [255, 0, 0, 255]);
        }
    }
}
