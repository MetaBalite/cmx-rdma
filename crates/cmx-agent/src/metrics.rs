//! Prometheus metrics endpoint.

use std::net::SocketAddr;

/// Install the Prometheus metrics recorder and start the HTTP exporter.
///
/// Metrics are served at `http://{addr}/metrics`. Call this once before
/// emitting any `metrics::counter!` / `metrics::gauge!` / `metrics::histogram!`.
pub fn install_metrics_recorder(addr: SocketAddr) -> anyhow::Result<()> {
    metrics_exporter_prometheus::PrometheusBuilder::new()
        .with_http_listener(addr)
        .install()
        .map_err(|e| anyhow::anyhow!("failed to install Prometheus exporter: {e}"))?;

    tracing::info!(%addr, "Prometheus metrics exporter started");
    Ok(())
}
