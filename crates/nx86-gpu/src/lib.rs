//! Renderer orchestration above the `nx86-vulkan` safe boundary.
//!
//! [`Renderer`] hides the host/target split behind one API: on Linux with a
//! Vulkan device it renders on the GPU; on the Apple Silicon dev host or
//! headless CI it falls back to a deterministic CPU renderer. Either way it
//! yields a [`Framebuffer`] of tightly packed `R8G8B8A8` bytes, reusing the
//! shared `nx86_testsuite::Framebuffer` type the GUI framebuffer view and the
//! runtime already consume. This mirrors the `cpal`-or-null fallback used by the
//! audio runtime, keeping the runtime able to produce a frame on every host.
//!
//! No raw `ash`/Vulkan handles cross this boundary — only plain RGBA bytes.

use nx86_testsuite::Framebuffer;
use nx86_vulkan::VulkanAvailability;

/// Default frame color (opaque dark slate) used as the render-target background.
pub const BACKGROUND: [u8; 4] = [18, 22, 33, 255];
/// Foreground color (opaque teal) used for the software test-card triangle.
pub const FOREGROUND: [u8; 4] = [64, 196, 180, 255];

/// Which backend produced (or would produce) a frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Backend {
    /// Hardware Vulkan, with the physical device name.
    Vulkan(String),
    /// Deterministic CPU fallback, with the reason Vulkan was unavailable.
    Software(String),
}

impl Backend {
    /// `true` when frames are produced on the GPU.
    #[must_use]
    pub fn is_hardware(&self) -> bool {
        matches!(self, Self::Vulkan(_))
    }

    /// Short human-readable label for status display.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Vulkan(name) => format!("Vulkan ({name})"),
            Self::Software(reason) => format!("Software fallback ({reason})"),
        }
    }
}

/// Selects a Vulkan device when present, else a deterministic CPU renderer.
pub struct Renderer {
    availability: VulkanAvailability,
    backend: Backend,
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

impl Renderer {
    /// Detect Vulkan once and pick a backend. Never panics.
    #[must_use]
    pub fn new() -> Self {
        let availability = VulkanAvailability::detect();
        let backend = match &availability {
            VulkanAvailability::Available(renderer) => {
                Backend::Vulkan(renderer.device_name().to_string())
            }
            VulkanAvailability::Unavailable { reason } => Backend::Software(reason.clone()),
        };
        Self {
            availability,
            backend,
        }
    }

    /// The backend in use.
    #[must_use]
    pub fn backend(&self) -> &Backend {
        &self.backend
    }

    /// `true` when frames are GPU-rendered.
    #[must_use]
    pub fn is_hardware(&self) -> bool {
        self.backend.is_hardware()
    }

    /// Render the Phase 48 demonstration frame at `width`×`height`.
    ///
    /// On the GPU this clears the offscreen target to [`BACKGROUND`] (a flat
    /// frame; geometry arrives with shader translation in Phase 49). The CPU
    /// fallback rasterizes a centered triangle over the background so the
    /// fallback frame is visibly a rendered image and is deterministic for
    /// tests. If a GPU render fails at runtime, it degrades to the CPU frame.
    #[must_use]
    pub fn render_demo(&self, width: u32, height: u32) -> Framebuffer {
        if let VulkanAvailability::Available(renderer) = &self.availability {
            let color = unorm(BACKGROUND);
            if let Ok(bytes) = renderer.render_clear(width, height, color) {
                return Framebuffer {
                    width,
                    height,
                    bytes,
                };
            }
        }
        software_test_card(width, height)
    }
}

/// Convert an `R8G8B8A8` color to normalized `0.0..=1.0` floats.
fn unorm(color: [u8; 4]) -> [f32; 4] {
    [
        f32::from(color[0]) / 255.0,
        f32::from(color[1]) / 255.0,
        f32::from(color[2]) / 255.0,
        f32::from(color[3]) / 255.0,
    ]
}

/// CPU-rasterize the deterministic test card: [`BACKGROUND`] filled with a
/// centered, upward-pointing [`FOREGROUND`] triangle.
#[must_use]
pub fn software_test_card(width: u32, height: u32) -> Framebuffer {
    let mut bytes = BACKGROUND.repeat(width as usize * height as usize);
    if width >= 3 && height >= 3 {
        rasterize_triangle(&mut bytes, width, height);
    }
    Framebuffer {
        width,
        height,
        bytes,
    }
}

/// Fill the centered triangle into `rgba8` using edge-function inside tests.
fn rasterize_triangle(rgba8: &mut [u8], width: u32, height: u32) {
    let w = width as f32;
    let h = height as f32;
    // Apex at top-center, base across the lower third.
    let apex = (w * 0.5, h * 0.15);
    let left = (w * 0.2, h * 0.85);
    let right = (w * 0.8, h * 0.85);

    for y in 0..height {
        for x in 0..width {
            let p = (x as f32 + 0.5, y as f32 + 0.5);
            if point_in_triangle(p, apex, left, right) {
                let idx = (y as usize * width as usize + x as usize) * 4;
                rgba8[idx..idx + 4].copy_from_slice(&FOREGROUND);
            }
        }
    }
}

/// Edge function: twice the signed area of triangle `(a, b, c)`.
fn edge(a: (f32, f32), b: (f32, f32), c: (f32, f32)) -> f32 {
    (c.0 - a.0) * (b.1 - a.1) - (c.1 - a.1) * (b.0 - a.0)
}

/// `true` when `p` is inside (or on the edge of) triangle `(a, b, c)`,
/// independent of winding order.
fn point_in_triangle(p: (f32, f32), a: (f32, f32), b: (f32, f32), c: (f32, f32)) -> bool {
    let d1 = edge(a, b, p);
    let d2 = edge(b, c, p);
    let d3 = edge(c, a, p);
    let has_neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let has_pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(has_neg && has_pos)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rendered_frame_has_expected_byte_length() {
        let frame = software_test_card(16, 12);
        assert_eq!(frame.width, 16);
        assert_eq!(frame.height, 12);
        assert_eq!(frame.bytes.len(), 16 * 12 * 4);
    }

    #[test]
    fn test_card_paints_background_corners_and_foreground_center() {
        let (w, h) = (32_u32, 32_u32);
        let frame = software_test_card(w, h);
        // Top-left corner is background.
        assert_eq!(&frame.bytes[0..4], &BACKGROUND);
        // Center pixel falls inside the triangle.
        let center = ((h / 2) as usize * w as usize + (w / 2) as usize) * 4;
        assert_eq!(&frame.bytes[center..center + 4], &FOREGROUND);
    }

    #[test]
    fn test_card_is_deterministic() {
        assert_eq!(software_test_card(24, 24), software_test_card(24, 24));
    }

    #[test]
    fn renderer_always_produces_a_frame_of_requested_size() {
        // On the dev host this exercises the software fallback; on Linux with a
        // device it exercises the GPU clear. Both must return the right size.
        let renderer = Renderer::new();
        let frame = renderer.render_demo(40, 30);
        assert_eq!(frame.width, 40);
        assert_eq!(frame.height, 30);
        assert_eq!(frame.bytes.len(), 40 * 30 * 4);
        assert!(!renderer.backend().label().is_empty());
    }
}
