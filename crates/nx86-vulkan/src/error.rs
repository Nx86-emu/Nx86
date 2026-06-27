//! Typed errors for the Vulkan boundary.
//!
//! Every fallible Vulkan operation returns [`VulkanError`] so that higher-level
//! crates (`nx86-gpu`, the runtime) can degrade gracefully — for example by
//! falling back to a deterministic software frame — instead of panicking. The
//! workspace lints forbid `unwrap`/`todo`, so the boundary never papers over a
//! failure with a panic.

use core::fmt;

/// A failure raised while loading, initializing, or driving Vulkan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VulkanError {
    /// The Vulkan loader could not be loaded (no driver/loader installed). This
    /// is the expected outcome on the Apple Silicon dev host and headless CI.
    LoaderUnavailable(String),
    /// Instance creation failed.
    InstanceCreation(String),
    /// No physical device exposed a usable graphics queue family.
    NoSuitableDevice,
    /// Logical device creation failed.
    DeviceCreation(String),
    /// A surface operation failed (windowed present path).
    Surface(String),
    /// A swapchain operation failed (create/acquire/present/recreate).
    Swapchain(String),
    /// A device memory allocation could not be satisfied.
    Allocation(String),
    /// Command recording, submission, or fence wait failed.
    Command(String),
    /// Frame presentation failed (device or surface loss, out-of-date).
    Present(String),
}

impl fmt::Display for VulkanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LoaderUnavailable(why) => write!(f, "vulkan loader unavailable: {why}"),
            Self::InstanceCreation(why) => write!(f, "vulkan instance creation failed: {why}"),
            Self::NoSuitableDevice => {
                write!(f, "no physical device exposes a graphics queue family")
            }
            Self::DeviceCreation(why) => write!(f, "vulkan device creation failed: {why}"),
            Self::Surface(why) => write!(f, "vulkan surface error: {why}"),
            Self::Swapchain(why) => write!(f, "vulkan swapchain error: {why}"),
            Self::Allocation(why) => write!(f, "vulkan allocation error: {why}"),
            Self::Command(why) => write!(f, "vulkan command error: {why}"),
            Self::Present(why) => write!(f, "vulkan present error: {why}"),
        }
    }
}

impl std::error::Error for VulkanError {}

/// Result alias for the Vulkan boundary.
pub type VulkanResult<T> = Result<T, VulkanError>;
