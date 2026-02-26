/// GPU device identifier.
pub type GpuDevice = u32;

/// Register a memory region for GPUDirect RDMA (stub: no-op).
pub fn register_gpu_memory(
    _device: GpuDevice,
    _ptr: *mut u8,
    _size: usize,
) -> Result<(), &'static str> {
    // Phase 2: Will call CUDA/NIXL APIs to register VRAM
    Ok(())
}
