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
}

impl Default for AgentSection {
    fn default() -> Self {
        Self {
            node_id: String::new(),
            listen_addr: default_listen_addr(),
            log_level: default_log_level(),
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
}

impl Default for MemorySection {
    fn default() -> Self {
        Self {
            total_size: default_total_size(),
            block_size: default_block_size(),
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
}

impl Default for MetadataSection {
    fn default() -> Self {
        Self {
            etcd_endpoints: default_etcd_endpoints(),
            key_prefix: default_key_prefix(),
            ttl_seconds: default_ttl_seconds(),
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

fn default_listen_addr() -> String {
    "0.0.0.0:50051".into()
}
fn default_log_level() -> String {
    "info".into()
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
fn default_transport_backend() -> String {
    "mock".into()
}
fn default_metrics_addr() -> String {
    "0.0.0.0:9090".into()
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
    }

    #[test]
    fn test_parse_toml() {
        let toml = r#"
            [agent]
            node_id = "test-node"
            listen_addr = "127.0.0.1:50051"

            [memory]
            total_size = 536870912
            block_size = 131072
        "#;

        let config: AgentConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.agent.node_id, "test-node");
        assert_eq!(config.memory.total_size, 512 * 1024 * 1024);
        assert_eq!(config.memory.block_size, 128 * 1024);
        // Defaults for missing sections
        assert_eq!(config.transport.backend, "mock");
    }
}
