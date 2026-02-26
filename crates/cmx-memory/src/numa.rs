/// NUMA node identifier.
pub type NumaNode = u32;

/// Get the NUMA node for a given CPU core (stub: returns 0).
pub fn numa_node_of_cpu(_cpu: u32) -> NumaNode {
    0
}

/// Get the preferred NUMA node for RDMA NIC (stub: returns 0).
pub fn preferred_numa_node() -> NumaNode {
    0
}
