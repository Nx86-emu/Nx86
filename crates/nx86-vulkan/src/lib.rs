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

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct VulkanBackend {
    info: VulkanBackendInfo,
}

impl VulkanBackend {
    #[must_use]
    pub fn new_placeholder() -> Self {
        Self {
            info: VulkanBackendInfo::current(),
        }
    }

    #[must_use]
    pub const fn info(&self) -> VulkanBackendInfo {
        self.info
    }
}

#[must_use]
pub fn backend_info() -> VulkanBackendInfo {
    VulkanBackendInfo::current()
}
