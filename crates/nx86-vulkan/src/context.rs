//! Vulkan instance + device bring-up.
//!
//! [`VulkanContext`] owns the loader entry, instance, the chosen physical
//! device, the logical device, and a graphics queue. It is the single place
//! that loads the Vulkan loader, so a host without a loader (the Apple Silicon
//! dev host, headless CI) fails here with [`VulkanError::LoaderUnavailable`]
//! and the rest of the stack falls back to a deterministic software frame.
//!
//! All raw `ash` handles stay private to this crate; the unsafe FFI is confined
//! to narrow `#[allow(unsafe_code)]` islands, each with a `SAFETY:` note, matching
//! the convention in `nx86-jit` and `nx86-vmm`.

use ash::vk;

use crate::error::{VulkanError, VulkanResult};

/// An initialized Vulkan device with a graphics-capable queue.
pub struct VulkanContext {
    // `entry` must outlive `instance`, and `instance` must outlive `device`;
    // declaration order plus the manual `Drop` keep teardown correct.
    entry: ash::Entry,
    instance: ash::Instance,
    physical_device: vk::PhysicalDevice,
    device: ash::Device,
    queue_family_index: u32,
    queue: vk::Queue,
    device_name: String,
    swapchain_enabled: bool,
}

impl VulkanContext {
    /// Load the Vulkan loader and bring up an instance, physical device, and a
    /// graphics queue. Returns [`VulkanError::LoaderUnavailable`] when no loader
    /// is present — the expected, non-fatal outcome on unsupported hosts.
    pub fn new() -> VulkanResult<Self> {
        let entry = load_entry()?;
        let instance = create_instance(&entry)?;
        let (physical_device, queue_family_index, device_name) = select_physical_device(&instance)?;
        let (device, swapchain_enabled) =
            create_device(&instance, physical_device, queue_family_index)?;
        // SAFETY: `queue_family_index` was selected from this device's reported
        // queue families and queue index 0 always exists for a created queue.
        #[allow(unsafe_code)]
        let queue = unsafe { device.get_device_queue(queue_family_index, 0) };
        Ok(Self {
            entry,
            instance,
            physical_device,
            device,
            queue_family_index,
            queue,
            device_name,
            swapchain_enabled,
        })
    }

    /// Whether the logical device was created with `VK_KHR_swapchain` enabled.
    /// The swapchain path must not be entered when this is `false`, or swapchain
    /// device commands would be called on a device that never enabled them.
    pub(crate) fn swapchain_enabled(&self) -> bool {
        self.swapchain_enabled
    }

    /// Human-readable name of the selected physical device.
    #[must_use]
    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    pub(crate) fn device(&self) -> &ash::Device {
        &self.device
    }

    pub(crate) fn physical_device(&self) -> vk::PhysicalDevice {
        self.physical_device
    }

    pub(crate) fn queue_family_index(&self) -> u32 {
        self.queue_family_index
    }

    pub(crate) fn queue(&self) -> vk::Queue {
        self.queue
    }

    /// Build the `VK_KHR_surface` instance loader used by the swapchain path.
    pub(crate) fn surface_instance(&self) -> ash::khr::surface::Instance {
        ash::khr::surface::Instance::new(&self.entry, &self.instance)
    }

    /// Build the `VK_KHR_swapchain` device loader used by the swapchain path.
    pub(crate) fn swapchain_device(&self) -> ash::khr::swapchain::Device {
        ash::khr::swapchain::Device::new(&self.instance, &self.device)
    }

    /// Find a memory type index satisfying `type_bits` with all `flags` set.
    pub(crate) fn find_memory_type(
        &self,
        type_bits: u32,
        flags: vk::MemoryPropertyFlags,
    ) -> VulkanResult<u32> {
        // SAFETY: `physical_device` came from this instance's enumeration.
        #[allow(unsafe_code)]
        let props = unsafe {
            self.instance
                .get_physical_device_memory_properties(self.physical_device)
        };
        for index in 0..props.memory_type_count {
            let supported = (type_bits & (1 << index)) != 0;
            let has_flags = props.memory_types[index as usize]
                .property_flags
                .contains(flags);
            if supported && has_flags {
                return Ok(index);
            }
        }
        Err(VulkanError::Allocation(format!(
            "no memory type matches bits {type_bits:#x} with flags {:#x}",
            flags.as_raw()
        )))
    }
}

impl Drop for VulkanContext {
    fn drop(&mut self) {
        // SAFETY: `device` and `instance` were created here. We wait for the
        // device to go idle before teardown so any outstanding GPU work (e.g. an
        // async swapchain present) completes first, then destroy the device
        // before the instance per the Vulkan spec. `device_wait_idle` failure is
        // best-effort during teardown and cannot be propagated from `drop`.
        #[allow(unsafe_code)]
        unsafe {
            let _ = self.device.device_wait_idle();
            self.device.destroy_device(None);
            self.instance.destroy_instance(None);
        }
    }
}

fn load_entry() -> VulkanResult<ash::Entry> {
    // SAFETY: `Entry::load` dynamically loads the system Vulkan loader. It is
    // safe to call once at startup; any failure (no loader installed) is
    // surfaced as a typed error rather than a panic.
    #[allow(unsafe_code)]
    let loaded = unsafe { ash::Entry::load() };
    loaded.map_err(|err| VulkanError::LoaderUnavailable(err.to_string()))
}

fn create_instance(entry: &ash::Entry) -> VulkanResult<ash::Instance> {
    let app_info = vk::ApplicationInfo::default()
        .application_name(c"nx86")
        .application_version(0)
        .engine_name(c"nx86")
        .api_version(vk::API_VERSION_1_3);
    let create_info = vk::InstanceCreateInfo::default().application_info(&app_info);
    // SAFETY: `create_info` borrows `app_info`, which outlives this call, and
    // requests no extensions or layers, so the pointers it carries are valid.
    #[allow(unsafe_code)]
    let result = unsafe { entry.create_instance(&create_info, None) };
    result.map_err(|err| VulkanError::InstanceCreation(err.to_string()))
}

fn select_physical_device(
    instance: &ash::Instance,
) -> VulkanResult<(vk::PhysicalDevice, u32, String)> {
    // SAFETY: `instance` is a live instance created above.
    #[allow(unsafe_code)]
    let devices = unsafe { instance.enumerate_physical_devices() }
        .map_err(|err| VulkanError::DeviceCreation(err.to_string()))?;
    for device in devices {
        // SAFETY: `device` came from this instance's enumeration.
        #[allow(unsafe_code)]
        let families = unsafe { instance.get_physical_device_queue_family_properties(device) };
        let graphics = families.iter().position(|family| {
            family.queue_flags.contains(vk::QueueFlags::GRAPHICS) && family.queue_count > 0
        });
        if let Some(index) = graphics {
            let name = physical_device_name(instance, device);
            return Ok((device, index as u32, name));
        }
    }
    Err(VulkanError::NoSuitableDevice)
}

fn physical_device_name(instance: &ash::Instance, device: vk::PhysicalDevice) -> String {
    // SAFETY: `device` came from this instance's enumeration.
    #[allow(unsafe_code)]
    let props = unsafe { instance.get_physical_device_properties(device) };
    let raw = props.device_name_as_c_str();
    match raw {
        Ok(name) => name.to_string_lossy().into_owned(),
        Err(_) => "unknown device".to_string(),
    }
}

/// Create the logical device, returning it plus whether `VK_KHR_swapchain` was
/// enabled. The caller records the flag so the swapchain path can refuse to run
/// on a device that fell back without the extension.
fn create_device(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
    queue_family_index: u32,
) -> VulkanResult<(ash::Device, bool)> {
    let priorities = [1.0_f32];
    let queue_info = vk::DeviceQueueCreateInfo::default()
        .queue_family_index(queue_family_index)
        .queue_priorities(&priorities);
    let queue_infos = [queue_info];

    // Prefer a device with `VK_KHR_swapchain` enabled so the windowed present
    // path works; fall back to a plain device (headless/offscreen only) when the
    // extension is unavailable, which keeps the deterministic frame path alive.
    let swapchain_ext = [ash::khr::swapchain::NAME.as_ptr()];
    let with_swapchain = vk::DeviceCreateInfo::default()
        .queue_create_infos(&queue_infos)
        .enabled_extension_names(&swapchain_ext);
    // SAFETY: `with_swapchain` borrows `queue_infos`/`swapchain_ext`, both of
    // which outlive this call, and `physical_device` is a valid handle.
    #[allow(unsafe_code)]
    if let Ok(device) = unsafe { instance.create_device(physical_device, &with_swapchain, None) } {
        return Ok((device, true));
    }

    let plain = vk::DeviceCreateInfo::default().queue_create_infos(&queue_infos);
    // SAFETY: `plain` borrows `queue_infos`, which outlives this call, and
    // `physical_device` is a valid handle from this instance's enumeration.
    #[allow(unsafe_code)]
    let result = unsafe { instance.create_device(physical_device, &plain, None) };
    result
        .map(|device| (device, false))
        .map_err(|err| VulkanError::DeviceCreation(err.to_string()))
}
