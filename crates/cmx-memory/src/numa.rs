/// NUMA node identifier.
pub type NumaNode = u32;

/// Get the NUMA node for a given CPU core.
#[cfg(feature = "numa")]
pub fn numa_node_of_cpu(cpu: u32) -> NumaNode {
    let mut cpu_id: u32 = 0;
    let mut node_id: u32 = 0;
    // Safety: SYS_getcpu writes cpu and node IDs into the provided pointers.
    let ret = unsafe {
        libc::syscall(
            libc::SYS_getcpu,
            &mut cpu_id as *mut u32,
            &mut node_id as *mut u32,
            std::ptr::null_mut::<libc::c_void>(),
        )
    };
    if ret == 0 && cpu_id == cpu {
        node_id
    } else {
        0
    }
}

/// Get the NUMA node for a given CPU core (stub: returns 0).
#[cfg(not(feature = "numa"))]
pub fn numa_node_of_cpu(_cpu: u32) -> NumaNode {
    0
}

/// Get the preferred NUMA node for RDMA NIC.
#[cfg(feature = "numa")]
pub fn preferred_numa_node() -> NumaNode {
    // Try to read from sysfs for the first InfiniBand device.
    let path = "/sys/class/infiniband";
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let numa_path = entry.path().join("device/numa_node");
            if let Ok(content) = std::fs::read_to_string(&numa_path) {
                if let Ok(node) = content.trim().parse::<i32>() {
                    // -1 means no NUMA affinity
                    if node >= 0 {
                        return node as NumaNode;
                    }
                }
            }
        }
    }
    0
}

/// Get the preferred NUMA node for RDMA NIC (stub: returns 0).
#[cfg(not(feature = "numa"))]
pub fn preferred_numa_node() -> NumaNode {
    0
}

/// Bind a memory region to a specific NUMA node using mbind().
#[cfg(feature = "numa")]
pub fn numa_bind_memory(ptr: *mut u8, size: usize, node: NumaNode) -> Result<(), String> {
    const MPOL_BIND: i32 = 2;
    const MPOL_MF_MOVE: u32 = 2;

    // Build nodemask: a bitmask with bit `node` set.
    let mask_size = (node as usize / 64) + 1;
    let mut nodemask = vec![0u64; mask_size];
    nodemask[node as usize / 64] |= 1u64 << (node as usize % 64);

    // Safety: ptr is a valid mmap'd region of `size` bytes. mbind sets
    // the NUMA memory policy for the specified range.
    let ret = unsafe {
        libc::syscall(
            libc::SYS_mbind,
            ptr as *mut libc::c_void,
            size,
            MPOL_BIND,
            nodemask.as_ptr(),
            (mask_size * 64) + 1,
            MPOL_MF_MOVE,
        )
    };

    if ret == 0 {
        tracing::info!(node, size, "bound memory to NUMA node");
        Ok(())
    } else {
        Err(format!("mbind failed: {}", std::io::Error::last_os_error()))
    }
}

/// Bind a memory region to a specific NUMA node (stub: no-op).
#[cfg(not(feature = "numa"))]
pub fn numa_bind_memory(_ptr: *mut u8, _size: usize, _node: NumaNode) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_returns_zero() {
        assert_eq!(numa_node_of_cpu(0), 0);
        assert_eq!(preferred_numa_node(), 0);
    }

    #[test]
    fn stub_bind_succeeds() {
        assert!(numa_bind_memory(std::ptr::null_mut(), 0, 0).is_ok());
    }
}
