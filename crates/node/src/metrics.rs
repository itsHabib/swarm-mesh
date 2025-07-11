use anyhow::Result;
use mesh::NodeId;
use prometheus::{Encoder, Gauge, GaugeVec, Registry, TextEncoder};
use std::collections::HashMap;
use tokio::net::TcpListener;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use axum::{extract::State, http::StatusCode, response::Response, routing::get, Router};
use axum::body::Body;
use std::sync::Arc;

/// Metrics collector for mesh node statistics.
///
/// This struct manages Prometheus metrics for monitoring mesh network health,
/// including RTT measurements between peers and connection counts.
pub struct Metrics {
    /// Registry for all Prometheus metrics
    registry: Registry,
    /// Per-peer RTT measurements in seconds
    peer_rtt_current: GaugeVec,
    /// Per-peer minimum RTT in seconds
    peer_rtt_min: GaugeVec,
    /// Per-peer maximum RTT in seconds  
    peer_rtt_max: GaugeVec,
    /// Per-peer average RTT in seconds
    peer_rtt_avg: GaugeVec,
    /// Total number of connected peers for this node
    connected_peers_total: GaugeVec,
}


impl Metrics {
    /// Creates a new Metrics instance with all metric definitions.
    pub fn new() -> Result<Self> {
        let registry = Registry::new();

        let peer_rtt_current = GaugeVec::new(
            prometheus::Opts::new(
                "mesh_peer_rtt_current_seconds",
                "Current RTT to peer in seconds",
            ),
            &["local_node_id", "remote_node_id", "local_ip", "remote_ip"],
        )?;

        let peer_rtt_min = GaugeVec::new(
            prometheus::Opts::new(
                "mesh_peer_rtt_min_seconds",
                "Minimum RTT to peer in seconds",
            ),
            &["local_node_id", "remote_node_id", "local_ip", "remote_ip"],
        )?;

        let peer_rtt_max = GaugeVec::new(
            prometheus::Opts::new(
                "mesh_peer_rtt_max_seconds",
                "Maximum RTT to peer in seconds",
            ),
            &["local_node_id", "remote_node_id", "local_ip", "remote_ip"],
        )?;

        let peer_rtt_avg = GaugeVec::new(
            prometheus::Opts::new(
                "mesh_peer_rtt_avg_seconds",
                "Average RTT to peer in seconds",
            ),
            &["local_node_id", "remote_node_id", "local_ip", "remote_ip"],
        )?;

        let connected_peers_total = GaugeVec::new(
            prometheus::Opts::new(
                "mesh_connected_peers_total",
                "Total number of connected peers for this node",
            ),
            &["node_id", "node_ip"],
        )?;

        // Register all metrics
        registry.register(Box::new(peer_rtt_current.clone()))?;
        registry.register(Box::new(peer_rtt_min.clone()))?;
        registry.register(Box::new(peer_rtt_max.clone()))?;
        registry.register(Box::new(peer_rtt_avg.clone()))?;
        registry.register(Box::new(connected_peers_total.clone()))?;

        Ok(Self {
            registry,
            peer_rtt_current,
            peer_rtt_min,
            peer_rtt_max,
            peer_rtt_avg,
            connected_peers_total,
        })
    }

    /// Updates RTT metrics for a specific peer.
    pub fn update_peer_rtt(
        &self,
        local_node_id: u32,
        remote_node_id: u32,
        local_ip: &str,
        remote_ip: &str,
        rtt_stats: &mesh::RttStats,
    ) {
        let labels = &[
            &local_node_id.to_string(),
            &remote_node_id.to_string(),
            local_ip,
            remote_ip,
        ];

        // Update current RTT if available
        if let Some(current_rtt) = rtt_stats.current_rtt {
            self.peer_rtt_current
                .with_label_values(labels)
                .set(current_rtt.as_secs_f64());
        }

        self.peer_rtt_min
            .with_label_values(labels)
            .set(rtt_stats.min_rtt.as_secs_f64());

        self.peer_rtt_max
            .with_label_values(labels)
            .set(rtt_stats.max_rtt.as_secs_f64());

        self.peer_rtt_avg
            .with_label_values(labels)
            .set(rtt_stats.avg_rtt.as_secs_f64());
    }

    /// Updates the total connected peers count.
    pub fn update_connected_peers_count(&self, count: usize, node_ip: &str, node_id: NodeId) {
        let labels = &[&node_id.to_string(), node_ip];
        self.connected_peers_total
            .with_label_values(labels)
            .set(count as f64);
    }

    /// Renders all metrics in Prometheus text format.
    pub fn render(&self) -> Result<String> {
        let encoder = TextEncoder::new();
        let metric_families = self.registry.gather();
        let mut buffer = Vec::new();
        encoder.encode(&metric_families, &mut buffer)?;
        Ok(String::from_utf8(buffer)?)
    }

    /// Starts a simple HTTP server to expose metrics on the /metrics endpoint.
    pub async fn serve(metrics: Arc<Metrics>, port: u16) -> Result<()> {
        let app = Router::new()
            .route("/metrics", get(metrics_handler))
            .with_state(metrics);

        let listener = TcpListener::bind(format!("0.0.0.0:{}", port)).await?;
        tracing::info!("Metrics server listening on http://0.0.0.0:{}/metrics", port);

        axum::serve(listener, app).await?;
        Ok(())
    }
}

async fn metrics_handler(State(metrics): State<Arc<Metrics>>) -> Result<String, StatusCode> {
    metrics.render().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}
