//! Windowed-present swapchain backend (`VK_KHR_swapchain`).
//!
//! This is the real swapchain implementation: surface-capability queries,
//! format/present-mode selection, swapchain create/recreate, image retrieval,
//! image acquisition, and queue present. It is driven by a `vk::SurfaceKHR`
//! supplied by the caller's windowing layer.
//!
//! The product runs on Linux `x86_64-v4`; the swapchain path executes there with
//! a real surface. On the Apple Silicon dev host there is no Vulkan loader, so
//! [`crate::VulkanContext`] reports `Unavailable` and this module is never
//! entered. Live windowing onto the egui/Glow GUI is intentionally not wired in
//! this phase — the runtime surfaces frames as RGBA8 (see `offscreen`).
//!
//! This backend is compiled and ready but not yet driven by a live window, so
//! its items are `#![allow(dead_code)]` until the Linux windowing layer calls
//! them. Keeping it crate-internal also keeps raw `ash` handles out of the
//! public API, per the boundary rule.
#![allow(dead_code)]

use ash::vk;

use crate::context::VulkanContext;
use crate::error::{VulkanError, VulkanResult};

/// A created swapchain plus the metadata needed to render into and present its
/// images.
pub struct Swapchain {
    loader: ash::khr::swapchain::Device,
    handle: vk::SwapchainKHR,
    images: Vec<vk::Image>,
    format: vk::Format,
    extent: vk::Extent2D,
}

impl Swapchain {
    /// Create a swapchain for `surface` sized to `extent`.
    ///
    /// `surface` comes from the caller's windowing layer (for example via
    /// `ash-window` on Linux). The owning [`VulkanContext`] must have been
    /// created with `VK_KHR_swapchain` enabled.
    pub fn new(
        ctx: &VulkanContext,
        surface: vk::SurfaceKHR,
        extent: vk::Extent2D,
    ) -> VulkanResult<Self> {
        if !ctx.swapchain_enabled() {
            // The device fell back without `VK_KHR_swapchain`; calling swapchain
            // device commands on it is undefined behavior, so refuse cleanly.
            return Err(VulkanError::Swapchain(
                "device created without VK_KHR_swapchain".to_string(),
            ));
        }
        let surface_loader = ctx.surface_instance();
        let physical_device = ctx.physical_device();
        // SAFETY: `physical_device` and `surface` are live handles from this
        // instance and the caller's windowing layer.
        #[allow(unsafe_code)]
        let capabilities = unsafe {
            surface_loader.get_physical_device_surface_capabilities(physical_device, surface)
        }
        .map_err(|err| VulkanError::Surface(format!("capabilities: {err}")))?;
        // SAFETY: same handle validity as above.
        #[allow(unsafe_code)]
        let formats =
            unsafe { surface_loader.get_physical_device_surface_formats(physical_device, surface) }
                .map_err(|err| VulkanError::Surface(format!("formats: {err}")))?;
        // SAFETY: same handle validity as above.
        #[allow(unsafe_code)]
        let present_modes = unsafe {
            surface_loader.get_physical_device_surface_present_modes(physical_device, surface)
        }
        .map_err(|err| VulkanError::Surface(format!("present modes: {err}")))?;

        let surface_format = select_format(&formats)?;
        let present_mode = select_present_mode(&present_modes);
        let image_extent = clamp_extent(extent, &capabilities);
        let image_count = select_image_count(&capabilities);

        let create_info = vk::SwapchainCreateInfoKHR::default()
            .surface(surface)
            .min_image_count(image_count)
            .image_format(surface_format.format)
            .image_color_space(surface_format.color_space)
            .image_extent(image_extent)
            .image_array_layers(1)
            .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_DST)
            .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
            .pre_transform(capabilities.current_transform)
            .composite_alpha(vk::CompositeAlphaFlagsKHR::OPAQUE)
            .present_mode(present_mode)
            .clipped(true);

        let loader = ctx.swapchain_device();
        // SAFETY: `create_info` borrows `surface`, which the caller keeps alive,
        // and the loader was built from this context's instance/device.
        #[allow(unsafe_code)]
        let handle = unsafe { loader.create_swapchain(&create_info, None) }
            .map_err(|err| VulkanError::Swapchain(format!("create: {err}")))?;
        // SAFETY: `handle` was just created by `loader`.
        #[allow(unsafe_code)]
        let images = unsafe { loader.get_swapchain_images(handle) }
            .map_err(|err| VulkanError::Swapchain(format!("get images: {err}")))?;

        Ok(Self {
            loader,
            handle,
            images,
            format: surface_format.format,
            extent: image_extent,
        })
    }

    /// Swapchain image color format.
    pub fn format(&self) -> vk::Format {
        self.format
    }

    /// Swapchain image extent in pixels.
    pub fn extent(&self) -> vk::Extent2D {
        self.extent
    }

    /// Number of images in the swapchain.
    #[must_use]
    pub fn image_count(&self) -> usize {
        self.images.len()
    }

    /// Acquire the next presentable image index, signalling `semaphore` when the
    /// image is ready. Returns the image index and whether the swapchain is
    /// suboptimal (caller should recreate at a convenient time).
    pub fn acquire_next(&self, semaphore: vk::Semaphore) -> VulkanResult<(u32, bool)> {
        // SAFETY: `handle` and `semaphore` are live handles owned by this device.
        #[allow(unsafe_code)]
        let result = unsafe {
            self.loader
                .acquire_next_image(self.handle, u64::MAX, semaphore, vk::Fence::null())
        };
        result.map_err(|err| VulkanError::Swapchain(format!("acquire: {err}")))
    }

    /// Present image `image_index` on `queue` after `wait` is signalled.
    pub fn present(
        &self,
        queue: vk::Queue,
        image_index: u32,
        wait: vk::Semaphore,
    ) -> VulkanResult<()> {
        let wait_semaphores = [wait];
        let swapchains = [self.handle];
        let image_indices = [image_index];
        let info = vk::PresentInfoKHR::default()
            .wait_semaphores(&wait_semaphores)
            .swapchains(&swapchains)
            .image_indices(&image_indices);
        // SAFETY: all borrowed slices outlive the call and reference live handles.
        #[allow(unsafe_code)]
        let result = unsafe { self.loader.queue_present(queue, &info) };
        result
            .map(|_| ())
            .map_err(|err| VulkanError::Present(format!("queue_present: {err}")))
    }

    /// Recreate the swapchain at a new `extent` (e.g. after a window resize),
    /// destroying the previous swapchain (via `Drop`) first.
    pub fn recreate(
        self,
        ctx: &VulkanContext,
        surface: vk::SurfaceKHR,
        extent: vk::Extent2D,
    ) -> VulkanResult<Self> {
        drop(self);
        Self::new(ctx, surface, extent)
    }
}

impl Drop for Swapchain {
    fn drop(&mut self) {
        // SAFETY: `handle` belongs to `loader`; the owning `VulkanContext` waits
        // for the device to go idle before its own teardown, so no present is in
        // flight against this swapchain once it is dropped. Swapchain images are
        // owned by the swapchain and freed by `destroy_swapchain`.
        #[allow(unsafe_code)]
        unsafe {
            self.loader.destroy_swapchain(self.handle, None);
        }
    }
}

fn select_format(formats: &[vk::SurfaceFormatKHR]) -> VulkanResult<vk::SurfaceFormatKHR> {
    if formats.is_empty() {
        return Err(VulkanError::Surface("no surface formats".to_string()));
    }
    let preferred = formats.iter().copied().find(|format| {
        format.format == vk::Format::B8G8R8A8_UNORM
            && format.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR
    });
    Ok(preferred.unwrap_or(formats[0]))
}

fn select_present_mode(modes: &[vk::PresentModeKHR]) -> vk::PresentModeKHR {
    if modes.contains(&vk::PresentModeKHR::MAILBOX) {
        vk::PresentModeKHR::MAILBOX
    } else {
        // FIFO is required to be supported by the Vulkan spec.
        vk::PresentModeKHR::FIFO
    }
}

fn select_image_count(capabilities: &vk::SurfaceCapabilitiesKHR) -> u32 {
    let desired = capabilities.min_image_count + 1;
    if capabilities.max_image_count > 0 {
        desired.min(capabilities.max_image_count)
    } else {
        desired
    }
}

fn clamp_extent(
    requested: vk::Extent2D,
    capabilities: &vk::SurfaceCapabilitiesKHR,
) -> vk::Extent2D {
    // A `current_extent` of u32::MAX means the surface lets us pick the size.
    if capabilities.current_extent.width != u32::MAX {
        return capabilities.current_extent;
    }
    vk::Extent2D {
        width: requested.width.clamp(
            capabilities.min_image_extent.width,
            capabilities.max_image_extent.width,
        ),
        height: requested.height.clamp(
            capabilities.min_image_extent.height,
            capabilities.max_image_extent.height,
        ),
    }
}
