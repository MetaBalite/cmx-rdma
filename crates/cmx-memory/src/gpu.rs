/// GPU device identifier.
pub type GpuDevice = u32;

/// Register a memory region for GPUDirect RDMA.
///
/// When the `gpu` feature is enabled, calls `cudaHostRegister` to pin
/// host memory for GPUDirect access. Without the feature, this is a no-op.
#[cfg(feature = "gpu")]
pub fn register_gpu_memory(_device: GpuDevice, ptr: *mut u8, size: usize) -> Result<(), String> {
    // cudaHostRegisterPortable = 1, cudaHostRegisterMapped = 2
    const CUDA_HOST_REGISTER_PORTABLE: u32 = 1;
    const CUDA_HOST_REGISTER_MAPPED: u32 = 2;

    extern "C" {
        fn cudaHostRegister(ptr: *mut std::ffi::c_void, size: usize, flags: u32) -> i32;
    }

    let flags = CUDA_HOST_REGISTER_PORTABLE | CUDA_HOST_REGISTER_MAPPED;
    // Safety: ptr must be a valid host memory pointer of `size` bytes.
    let ret = unsafe { cudaHostRegister(ptr as *mut std::ffi::c_void, size, flags) };
    if ret == 0 {
        tracing::info!(size, "registered host memory for GPUDirect");
        Ok(())
    } else {
        Err(format!("cudaHostRegister failed with error code {ret}"))
    }
}

/// Register a memory region for GPUDirect RDMA (stub: no-op).
#[cfg(not(feature = "gpu"))]
pub fn register_gpu_memory(
    _device: GpuDevice,
    _ptr: *mut u8,
    _size: usize,
) -> Result<(), &'static str> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_register_succeeds() {
        assert!(register_gpu_memory(0, std::ptr::null_mut(), 0).is_ok());
    }
}
