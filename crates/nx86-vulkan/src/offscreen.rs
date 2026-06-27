//! Offscreen render-to-image: the deterministic, host-independent frame path.
//!
//! This renders into a device image and reads the result back as `R8G8B8A8`
//! bytes, which is the artifact behind the Phase 48 exit criterion ("runtime
//! displays a rendered frame"). It uses only core Vulkan transfer/clear
//! commands — no graphics pipeline or shaders — so it stays robust until shader
//! translation lands in Phase 49. The produced bytes match the layout of
//! `nx86_testsuite::Framebuffer`, so the existing GUI framebuffer view displays
//! them unchanged.

use ash::vk;

use crate::context::VulkanContext;
use crate::error::{VulkanError, VulkanResult};

const BYTES_PER_PIXEL: usize = 4;

/// Render a flat clear color into a `width`×`height` offscreen image and read it
/// back as tightly packed `R8G8B8A8` bytes (`width * height * 4` long).
///
/// `color` is RGBA in the `0.0..=1.0` range. The whole submission is fenced and
/// waited on before readback, so the returned buffer is complete.
pub fn render_clear(
    ctx: &VulkanContext,
    width: u32,
    height: u32,
    color: [f32; 4],
) -> VulkanResult<Vec<u8>> {
    if width == 0 || height == 0 {
        return Err(VulkanError::Command("zero-sized render target".to_string()));
    }
    let device = ctx.device();
    let buffer_len = width as usize * height as usize * BYTES_PER_PIXEL;

    // `res` owns every Vulkan object created below and frees them in `Drop`, so a
    // failure at any step (creation or the record/submit/readback sequence)
    // cannot leak — earlier code only freed on the success path.
    let mut res = FrameResources::new(device);
    res.image = create_color_image(ctx, width, height)?;
    res.image_memory = bind_image_memory(ctx, res.image)?;
    res.buffer = create_readback_buffer(ctx, buffer_len as u64)?;
    res.buffer_memory = bind_buffer_memory(ctx, res.buffer)?;
    res.pool = create_command_pool(ctx)?;
    let command_buffer = allocate_command_buffer(ctx, res.pool)?;

    record_clear_and_copy(
        ctx,
        command_buffer,
        res.image,
        res.buffer,
        width,
        height,
        color,
    )?;
    submit_and_wait(ctx, command_buffer)?;
    read_back(ctx, res.buffer_memory, buffer_len)
    // `res` is dropped here, freeing all resources on both the success and the
    // error paths above.
}

/// Owns the per-frame Vulkan objects so they are freed on every exit path,
/// including early `?` returns. Unset handles stay `null`, and destroying a null
/// Vulkan handle is a defined no-op, so partial construction is safe to drop.
struct FrameResources<'a> {
    device: &'a ash::Device,
    image: vk::Image,
    image_memory: vk::DeviceMemory,
    buffer: vk::Buffer,
    buffer_memory: vk::DeviceMemory,
    pool: vk::CommandPool,
}

impl<'a> FrameResources<'a> {
    fn new(device: &'a ash::Device) -> Self {
        Self {
            device,
            image: vk::Image::null(),
            image_memory: vk::DeviceMemory::null(),
            buffer: vk::Buffer::null(),
            buffer_memory: vk::DeviceMemory::null(),
            pool: vk::CommandPool::null(),
        }
    }
}

impl Drop for FrameResources<'_> {
    fn drop(&mut self) {
        // SAFETY: `submit_and_wait` waits on its fence before returning, so on the
        // success path no GPU work references these objects; on an error path the
        // submission either never ran or has completed/failed. Every handle was
        // created from `device`, and destroying a null handle is a defined no-op,
        // so freeing a partially constructed set is sound. Destroying the command
        // pool also frees its command buffers.
        #[allow(unsafe_code)]
        unsafe {
            self.device.destroy_command_pool(self.pool, None);
            self.device.destroy_buffer(self.buffer, None);
            self.device.free_memory(self.buffer_memory, None);
            self.device.destroy_image(self.image, None);
            self.device.free_memory(self.image_memory, None);
        }
    }
}

fn color_subresource_range() -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .base_mip_level(0)
        .level_count(1)
        .base_array_layer(0)
        .layer_count(1)
}

fn create_color_image(ctx: &VulkanContext, width: u32, height: u32) -> VulkanResult<vk::Image> {
    let info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(vk::Format::R8G8B8A8_UNORM)
        .extent(vk::Extent3D {
            width,
            height,
            depth: 1,
        })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::TRANSFER_SRC)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .initial_layout(vk::ImageLayout::UNDEFINED);
    // SAFETY: `info` is fully initialized and references no external pointers.
    #[allow(unsafe_code)]
    let result = unsafe { ctx.device().create_image(&info, None) };
    result.map_err(|err| VulkanError::Command(format!("create_image: {err}")))
}

fn bind_image_memory(ctx: &VulkanContext, image: vk::Image) -> VulkanResult<vk::DeviceMemory> {
    let device = ctx.device();
    // SAFETY: `image` was just created from `device`.
    #[allow(unsafe_code)]
    let requirements = unsafe { device.get_image_memory_requirements(image) };
    let type_index = ctx.find_memory_type(
        requirements.memory_type_bits,
        vk::MemoryPropertyFlags::DEVICE_LOCAL,
    )?;
    let info = vk::MemoryAllocateInfo::default()
        .allocation_size(requirements.size)
        .memory_type_index(type_index);
    // SAFETY: `info` is fully initialized; `image` belongs to `device`.
    #[allow(unsafe_code)]
    let memory = unsafe { device.allocate_memory(&info, None) }
        .map_err(|err| VulkanError::Allocation(format!("image memory: {err}")))?;
    // SAFETY: `memory` was sized from `image`'s requirements and offset 0 is valid.
    // On bind failure we free the just-allocated memory before returning, since
    // the caller never receives the handle and cannot free it itself.
    #[allow(unsafe_code)]
    if let Err(err) = unsafe { device.bind_image_memory(image, memory, 0) } {
        #[allow(unsafe_code)]
        unsafe {
            device.free_memory(memory, None);
        }
        return Err(VulkanError::Allocation(format!("bind_image_memory: {err}")));
    }
    Ok(memory)
}

fn create_readback_buffer(ctx: &VulkanContext, size: u64) -> VulkanResult<vk::Buffer> {
    let info = vk::BufferCreateInfo::default()
        .size(size)
        .usage(vk::BufferUsageFlags::TRANSFER_DST)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    // SAFETY: `info` is fully initialized and references no external pointers.
    #[allow(unsafe_code)]
    let result = unsafe { ctx.device().create_buffer(&info, None) };
    result.map_err(|err| VulkanError::Command(format!("create_buffer: {err}")))
}

fn bind_buffer_memory(ctx: &VulkanContext, buffer: vk::Buffer) -> VulkanResult<vk::DeviceMemory> {
    let device = ctx.device();
    // SAFETY: `buffer` was just created from `device`.
    #[allow(unsafe_code)]
    let requirements = unsafe { device.get_buffer_memory_requirements(buffer) };
    let type_index = ctx.find_memory_type(
        requirements.memory_type_bits,
        vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
    )?;
    let info = vk::MemoryAllocateInfo::default()
        .allocation_size(requirements.size)
        .memory_type_index(type_index);
    // SAFETY: `info` is fully initialized; `buffer` belongs to `device`.
    #[allow(unsafe_code)]
    let memory = unsafe { device.allocate_memory(&info, None) }
        .map_err(|err| VulkanError::Allocation(format!("buffer memory: {err}")))?;
    // SAFETY: `memory` was sized from `buffer`'s requirements and offset 0 is valid.
    // On bind failure we free the just-allocated memory before returning, since
    // the caller never receives the handle and cannot free it itself.
    #[allow(unsafe_code)]
    if let Err(err) = unsafe { device.bind_buffer_memory(buffer, memory, 0) } {
        #[allow(unsafe_code)]
        unsafe {
            device.free_memory(memory, None);
        }
        return Err(VulkanError::Allocation(format!(
            "bind_buffer_memory: {err}"
        )));
    }
    Ok(memory)
}

fn create_command_pool(ctx: &VulkanContext) -> VulkanResult<vk::CommandPool> {
    let info = vk::CommandPoolCreateInfo::default()
        .queue_family_index(ctx.queue_family_index())
        .flags(vk::CommandPoolCreateFlags::TRANSIENT);
    // SAFETY: `info` is fully initialized; the queue family index came from this device.
    #[allow(unsafe_code)]
    let result = unsafe { ctx.device().create_command_pool(&info, None) };
    result.map_err(|err| VulkanError::Command(format!("create_command_pool: {err}")))
}

fn allocate_command_buffer(
    ctx: &VulkanContext,
    pool: vk::CommandPool,
) -> VulkanResult<vk::CommandBuffer> {
    let info = vk::CommandBufferAllocateInfo::default()
        .command_pool(pool)
        .level(vk::CommandBufferLevel::PRIMARY)
        .command_buffer_count(1);
    // SAFETY: `info` references `pool`, which belongs to this device.
    #[allow(unsafe_code)]
    let buffers = unsafe { ctx.device().allocate_command_buffers(&info) }
        .map_err(|err| VulkanError::Command(format!("allocate_command_buffers: {err}")))?;
    buffers
        .into_iter()
        .next()
        .ok_or_else(|| VulkanError::Command("no command buffer allocated".to_string()))
}

fn record_clear_and_copy(
    ctx: &VulkanContext,
    command_buffer: vk::CommandBuffer,
    image: vk::Image,
    buffer: vk::Buffer,
    width: u32,
    height: u32,
    color: [f32; 4],
) -> VulkanResult<()> {
    let device = ctx.device();
    let begin =
        vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
    let range = color_subresource_range();

    let to_transfer_dst = vk::ImageMemoryBarrier::default()
        .old_layout(vk::ImageLayout::UNDEFINED)
        .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
        .src_access_mask(vk::AccessFlags::empty())
        .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image)
        .subresource_range(range);
    let to_transfer_src = vk::ImageMemoryBarrier::default()
        .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
        .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
        .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
        .dst_access_mask(vk::AccessFlags::TRANSFER_READ)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image)
        .subresource_range(range);

    let clear_value = vk::ClearColorValue { float32: color };
    let copy = vk::BufferImageCopy::default()
        .buffer_offset(0)
        .buffer_row_length(0)
        .buffer_image_height(0)
        .image_subresource(
            vk::ImageSubresourceLayers::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .mip_level(0)
                .base_array_layer(0)
                .layer_count(1),
        )
        .image_offset(vk::Offset3D { x: 0, y: 0, z: 0 })
        .image_extent(vk::Extent3D {
            width,
            height,
            depth: 1,
        });

    // SAFETY: all handles belong to `device`; the recorded commands form a valid
    // UNDEFINED -> TRANSFER_DST -> clear -> TRANSFER_SRC -> copy sequence and the
    // command buffer was freshly allocated, so recording into it is sound.
    #[allow(unsafe_code)]
    unsafe {
        device
            .begin_command_buffer(command_buffer, &begin)
            .map_err(|err| VulkanError::Command(format!("begin: {err}")))?;
        device.cmd_pipeline_barrier(
            command_buffer,
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::TRANSFER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[to_transfer_dst],
        );
        device.cmd_clear_color_image(
            command_buffer,
            image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            &clear_value,
            &[range],
        );
        device.cmd_pipeline_barrier(
            command_buffer,
            vk::PipelineStageFlags::TRANSFER,
            vk::PipelineStageFlags::TRANSFER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[to_transfer_src],
        );
        device.cmd_copy_image_to_buffer(
            command_buffer,
            image,
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            buffer,
            &[copy],
        );
        device
            .end_command_buffer(command_buffer)
            .map_err(|err| VulkanError::Command(format!("end: {err}")))?;
    }
    Ok(())
}

fn submit_and_wait(ctx: &VulkanContext, command_buffer: vk::CommandBuffer) -> VulkanResult<()> {
    let device = ctx.device();
    // SAFETY: a default fence info is valid.
    #[allow(unsafe_code)]
    let fence = unsafe { device.create_fence(&vk::FenceCreateInfo::default(), None) }
        .map_err(|err| VulkanError::Command(format!("create_fence: {err}")))?;
    let command_buffers = [command_buffer];
    let submit = vk::SubmitInfo::default().command_buffers(&command_buffers);
    // SAFETY: `command_buffer` was recorded and ended above; `fence` and the
    // queue belong to this device. We wait on the fence before returning, so the
    // command buffer outlives its execution.
    #[allow(unsafe_code)]
    let outcome = unsafe {
        device
            .queue_submit(ctx.queue(), &[submit], fence)
            .map_err(|err| VulkanError::Command(format!("queue_submit: {err}")))
            .and_then(|()| {
                device
                    .wait_for_fences(&[fence], true, u64::MAX)
                    .map_err(|err| VulkanError::Command(format!("wait_for_fences: {err}")))
            })
    };
    // SAFETY: the fence is no longer in use once the wait above returns.
    #[allow(unsafe_code)]
    unsafe {
        device.destroy_fence(fence, None);
    }
    outcome
}

fn read_back(ctx: &VulkanContext, memory: vk::DeviceMemory, len: usize) -> VulkanResult<Vec<u8>> {
    let device = ctx.device();
    let mut pixels = vec![0_u8; len];
    // SAFETY: `memory` is host-visible/coherent and `len` bytes long; we map the
    // full range, copy out exactly `len` bytes, then unmap. The mapping outlives
    // the copy.
    #[allow(unsafe_code)]
    unsafe {
        let ptr = device
            .map_memory(memory, 0, len as u64, vk::MemoryMapFlags::empty())
            .map_err(|err| VulkanError::Allocation(format!("map_memory: {err}")))?;
        core::ptr::copy_nonoverlapping(ptr.cast::<u8>(), pixels.as_mut_ptr(), len);
        device.unmap_memory(memory);
    }
    Ok(pixels)
}
