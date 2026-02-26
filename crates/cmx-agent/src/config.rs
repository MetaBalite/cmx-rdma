//! TOML configuration loading for cmx-agent.

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct AgentConfig {
    #[serde(default)]
    pub agent: AgentSection,
    #[serde(default)]
    pub memory: MemorySection,
    #[serde(default)]
    pub metadata: MetadataSection,
    #[serde(default)]
    pub transport: TransportSection,
    #[serde(default)]
    pub metrics: MetricsSection,
    #[serde(default)]
    pub placement: PlacementSection,
}

#[derive(Debug, Deserialize)]
pub struct AgentSection {
    /// Unique node identifier. Auto-generated from hostname if empty.
    #[serde(default)]
    pub node_id: String,
    /// gRPC listen address.
    #[serde(default = "default_listen_addr")]
    pub listen_addr: String,
    /// Log level.
    #[serde(default = "default_log_level")]
    pub log_level: String,
    /// Log format: "text" or "json".
    #[serde(default = "default_log_format")]
    pub log_format: String,
}

impl Default for AgentSection {
    fn default() -> Self {
        Self {
            node_id: String::new(),
            listen_addr: default_listen_addr(),
            log_level: default_log_level(),
            log_format: default_log_format(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct MemorySection {
    /// Total memory pool size in bytes (default: 1 GiB).
    #[serde(default = "default_total_size")]
    pub total_size: usize,
    /// Block size in bytes (default: 256 KiB).
    #[serde(default = "default_block_size")]
    pub block_size: usize,
    /// NUMA node to bind memory to (None = OS default).
    #[serde(default)]
    pub numa_node: Option<u32>,
    /// Memory pressure warning threshold (fraction of pool used, default 0.75).
    #[serde(default = "default_pressure_warn")]
    pub pressure_warn: f64,
    /// Memory pressure critical threshold (default 0.90).
    #[serde(default = "default_pressure_critical")]
    pub pressure_critical: f64,
    /// Memory pressure reject threshold (default 0.95).
    #[serde(default = "default_pressure_reject")]
    pub pressure_reject: f64,
}

impl Default for MemorySection {
    fn default() -> Self {
        Self {
            total_size: default_total_size(),
            block_size: default_block_size(),
            numa_node: None,
            pressure_warn: default_pressure_warn(),
            pressure_critical: default_pressure_critical(),
            pressure_reject: default_pressure_reject(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct MetadataSection {
    /// etcd endpoints.
    #[serde(default = "default_etcd_endpoints")]
    pub etcd_endpoints: Vec<String>,
    /// Key prefix for metadata in etcd.
    #[serde(default = "default_key_prefix")]
    pub key_prefix: String,
    /// TTL for metadata entries in seconds.
    #[serde(default = "default_ttl_seconds")]
    pub ttl_seconds: u64,
    /// Lease TTL for node registration in seconds.
    #[serde(default = "default_lease_ttl_seconds")]
    pub lease_ttl_seconds: u64,
}

impl Default for MetadataSection {
    fn default() -> Self {
        Self {
            etcd_endpoints: default_etcd_endpoints(),
            key_prefix: default_key_prefix(),
            ttl_seconds: default_ttl_seconds(),
            lease_ttl_seconds: default_lease_ttl_seconds(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct TransportSection {
    /// Transport backend: "nixl" or "mock".
    #[serde(default = "default_transport_backend")]
    pub backend: String,
}

impl Default for TransportSection {
    fn default() -> Self {
        Self {
            backend: default_transport_backend(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct MetricsSection {
    /// Prometheus metrics listen address.
    #[serde(default = "default_metrics_addr")]
    pub listen_addr: String,
}

impl Default for MetricsSection {
    fn default() -> Self {
        Self {
            listen_addr: default_metrics_addr(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct PlacementSection {
    /// Number of virtual nodes per physical node in the hash ring.
    #[serde(default = "default_vnodes_per_node")]
    pub vnodes_per_node: u32,
}

impl Default for PlacementSection {
    fn default() -> Self {
        Self {
            vnodes_per_node: default_vnodes_per_node(),
        }
    }
}

fn default_listen_addr() -> String {
    "0.0.0.0:50051".into()
}
fn default_log_level() -> String {
    "info".into()
}
fn default_log_format() -> String {
    "text".into()
}
fn default_total_size() -> usize {
    1024 * 1024 * 1024 // 1 GiB
}
fn default_block_size() -> usize {
    256 * 1024 // 256 KiB
}
fn default_etcd_endpoints() -> Vec<String> {
    vec!["http://localhost:2379".into()]
}
fn default_key_prefix() -> String {
    "/cmx/blocks".into()
}
fn default_ttl_seconds() -> u64 {
    3600
}
fn default_lease_ttl_seconds() -> u64 {
    30
}
fn default_transport_backend() -> String {
    "mock".into()
}
fn default_metrics_addr() -> String {
    "0.0.0.0:9090".into()
}
fn default_vnodes_per_node() -> u32 {
    128
}
fn default_pressure_warn() -> f64 {
    0.75
}
fn default_pressure_critical() -> f64 {
    0.90
}
fn default_pressure_reject() -> f64 {
    0.95
}

impl AgentConfig {
    /// Load configuration from a TOML file. Missing fields use defaults.
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: AgentConfig = toml::from_str(&content)?;
        Ok(config)
    }

    /// Create a default configuration (useful for tests / when no file provided).
    pub fn default_config() -> Self {
        Self {
            agent: AgentSection::default(),
            memory: MemorySection::default(),
            metadata: MetadataSection::default(),
            transport: TransportSection::default(),
            metrics: MetricsSection::default(),
            placement: PlacementSection::default(),
        }
    }

    /// Resolve the node_id: use configured value, or fall back to hostname.
    pub fn resolve_node_id(&mut self) {
        if self.agent.node_id.is_empty() {
            // Check env var first (for Docker/K8s)
            if let Ok(id) = std::env::var("CMX_NODE_ID") {
                self.agent.node_id = id;
            } else {
                self.agent.node_id = gethostname();
            }
        }
    }
}

fn gethostname() -> String {
    let mut buf = [0u8; 256];
    // Safety: gethostname writes at most 256 bytes including NUL.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
    if rc == 0 {
        let nul_pos = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        String::from_utf8_lossy(&buf[..nul_pos]).into_owned()
    } else {
        "unknown".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = AgentConfig::default_config();
        assert_eq!(config.agent.listen_addr, "0.0.0.0:50051");
        assert_eq!(config.memory.block_size, 256 * 1024);
        assert_eq!(config.transport.backend, "mock");
        assert_eq!(config.agent.log_format, "text");
        assert_eq!(config.metadata.lease_ttl_seconds, 30);
        assert_eq!(config.placement.vnodes_per_node, 128);
        assert!((config.memory.pressure_warn - 0.75).abs() < f64::EPSILON);
    }

    #[test]
    fn test_parse_toml() {
        let toml = r#"
            [agent]
            node_id = "test-node"
            listen_addr = "127.0.0.1:50051"
            log_format = "json"

            [memory]
            total_size = 536870912
            block_size = 131072

            [placement]
            vnodes_per_node = 64
        "#;

        let config: AgentConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.agent.node_id, "test-node");
        assert_eq!(config.agent.log_format, "json");
        assert_eq!(config.memory.total_size, 512 * 1024 * 1024);
        assert_eq!(config.memory.block_size, 128 * 1024);
        assert_eq!(config.placement.vnodes_per_node, 64);
        // Defaults for missing sections
        assert_eq!(config.transport.backend, "mock");
    }

    #[test]
    fn test_parse_toml_with_pressure() {
        let toml = r#"
            [memory]
            pressure_warn = 0.70
            pressure_critical = 0.85
            pressure_reject = 0.92
        "#;

        let config: AgentConfig = toml::from_str(toml).unwrap();
        assert!((config.memory.pressure_warn - 0.70).abs() < f64::EPSILON);
        assert!((config.memory.pressure_critical - 0.85).abs() < f64::EPSILON);
        assert!((config.memory.pressure_reject - 0.92).abs() < f64::EPSILON);
    }
}
