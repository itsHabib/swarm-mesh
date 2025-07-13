use anyhow::Result;
use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use clap::Parser;
use registry::{Node, Registry};
use serde::{Deserialize, Serialize};
use serde_json;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::os::macos::raw::mode_t;
use std::sync::Arc;
use std::time::Duration;
use tokio::{net::TcpListener, sync::Mutex};
use tracing::{debug, error, info};

#[derive(Parser)]
#[command(name = "mesh-registry")]
#[command(
    about = "A mesh network registry service to support node discovery and Grafana graph panels"
)]
struct Args {
    #[arg(default_value = "http://localhost:9090")]
    prometheus_endpoint: String,
}

/// Node registration request structure.
///
/// This structure represents a request from a mesh node to register itself
/// with the registry service. It contains the essential information needed
/// to track and communicate with the node.
#[derive(Serialize, Deserialize, Debug)]
pub struct NodeRegistration {
    /// String representation of the node's unique identifier
    pub id: String,
    /// IP address where the node can be reached
    pub ip: String,
    /// Port number where the node exposes Prometheus metrics
    pub metrics_port: u16,
}

/// Graph data structure for Grafana node graph visualization.
///
/// This structure represents the complete graph topology for visualization
/// in Grafana dashboards. It contains both the nodes and their connections
/// (edges) in the mesh network.
#[derive(Serialize, Deserialize, Debug)]
pub struct GraphData {
    /// List of nodes in the mesh network
    pub nodes: Vec<GraphNode>,
    /// List of connections between nodes
    pub edges: Vec<Edge>,
}

/// Node representation for Grafana node graph visualization.
///
/// This structure represents a single node in the Grafana node graph panel.
/// It includes visual properties and statistics for display in the graph.
#[derive(Serialize, Deserialize, Debug)]
pub struct GraphNode {
    /// Unique identifier for the node
    pub id: String,
    /// Display title for the node
    pub title: String,
    /// Optional subtitle text
    pub subtitle: Option<String>,
    /// Primary statistic (typically connected peers count)
    pub mainStat: f64,
    /// Optional secondary statistic
    pub secondaryStat: Option<f64>,
    /// Detailed node information
    pub detail: NodeDetail,
    /// Optional color for the node visualization
    pub color: Option<String>,
}

/// Detailed information about a node for graph visualization.
///
/// This structure contains additional details about a node that are
/// displayed in the Grafana node graph panel when inspecting a node.
#[derive(Serialize, Deserialize, Debug)]
pub struct NodeDetail {
    /// IP address of the node
    pub ip: String,
    /// Node identifier
    pub id: String,
    /// Number of connected peers
    pub connected_peers: f64,
}

/// Edge representation for Grafana node graph visualization.
///
/// This structure represents a connection between two nodes in the Grafana
/// node graph panel. It includes RTT statistics and visual properties.
#[derive(Serialize, Deserialize, Debug)]
pub struct Edge {
    /// Unique identifier for the edge
    pub id: String,
    /// Source node identifier
    pub source: String,
    /// Target node identifier
    pub target: String,
    /// Primary statistic (typically current RTT)
    pub mainStat: f64,
    /// Optional secondary statistic (typically average RTT)
    pub secondaryStat: Option<f64>,
    /// Detailed edge information
    pub detail: EdgeDetail,
}

/// Detailed information about an edge for graph visualization.
///
/// This structure contains RTT statistics for a connection between two nodes
/// that are displayed in the Grafana node graph panel.
#[derive(Serialize, Deserialize, Debug)]
pub struct EdgeDetail {
    /// Current round-trip time in milliseconds
    pub rtt_current: Option<f64>,
    /// Average round-trip time in milliseconds
    pub rtt_avg: f64,
}

/// Prometheus service discovery target configuration.
///
/// This structure represents a target configuration for Prometheus service
/// discovery. It defines where Prometheus should scrape metrics from and
/// what labels to apply to the scraped metrics.
#[derive(Serialize, Deserialize, Debug)]
pub struct PrometheusTarget {
    /// List of target addresses (host:port) to scrape
    pub targets: Vec<String>,
    /// Labels to apply to metrics from these targets
    pub labels: PrometheusLabels,
}

/// Labels for Prometheus metrics from mesh nodes.
///
/// This structure defines the labels that Prometheus will apply to all
/// metrics scraped from a particular mesh node target.
#[derive(Serialize, Deserialize, Debug)]
pub struct PrometheusLabels {
    /// Job name for the Prometheus scrape configuration
    pub job: String,
    /// Node identifier for labeling metrics
    pub node_id: String,
    /// Node IP address for labeling metrics
    pub node_ip: String,
}

/// Response structure for Prometheus query API.
///
/// This structure represents the top-level response from the Prometheus
/// query API when requesting metric data.
#[derive(Deserialize, Debug)]
struct PrometheusResponse {
    /// The data portion of the Prometheus response
    data: PrometheusData,
}

/// Data portion of a Prometheus query response.
///
/// This structure contains the actual metric results from a Prometheus query.
#[derive(Deserialize, Debug)]
struct PrometheusData {
    /// Array of metric results from the query
    result: Vec<PrometheusResult>,
}

/// Individual metric result from a Prometheus query.
///
/// This structure represents a single metric result, including its labels
/// and current value.
#[derive(Deserialize, Debug)]
struct PrometheusResult {
    /// Labels associated with this metric
    metric: HashMap<String, String>,
    /// Value tuple containing [timestamp, value]
    value: [serde_json::Value; 2],
}

/// Application state for the mesh registry service.
///
/// This structure holds the shared state and configuration for the registry
/// service, including HTTP client, Prometheus endpoint, and node registry.
#[derive(Clone)]
pub struct App {
    /// Prometheus server endpoint for querying metrics
    pub prometheus_endpoint: String,
    /// HTTP client for making requests to Prometheus
    pub http_client: reqwest::Client,
    /// Thread-safe registry of active mesh nodes
    pub registry: Arc<Mutex<Registry>>,
}

/// Main entry point for the mesh registry service.
///
/// This function initializes the registry service with all necessary components:
/// - HTTP client for Prometheus queries
/// - Node registry with 1-minute TTL
/// - Background cleanup task for stale nodes
/// - Web server on port 5000
#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    log::init_logging();

    let http_client = reqwest::Client::new();
    let registry = Arc::new(Mutex::new(Registry::new(Duration::from_secs(60))));

    let app = App {
        prometheus_endpoint: args.prometheus_endpoint,
        http_client,
        registry: registry.clone(),
    };

    // Start background task to clean up stale nodes
    let cleanup_registry = registry.clone();
    tokio::spawn(async move {
        cleanup_stale_nodes(cleanup_registry).await;
    });

    serve(app, 5000).await
}

/// Starts the HTTP server for the mesh registry service.
///
/// This function sets up all the HTTP routes and starts the server:
/// - `/graph` - GET endpoint for Grafana graph data
/// - `/prometheus/targets` - GET endpoint for Prometheus service discovery
/// - `/register` - POST endpoint for node registration
/// - `/heartbeat` - POST endpoint for node heartbeats
/// - `/` - GET endpoint for health checks
///
/// # Arguments
/// * `app` - Application state containing registry and HTTP client
/// * `port` - Port number to bind the server to
///
/// # Returns
/// Returns `Ok(())` when the server shuts down gracefully, or an error if startup fails.
async fn serve(app: App, port: u16) -> Result<()> {
    let router = Router::new()
        .route("/graph", get(graph_handler))
        .route("/prometheus/targets", get(prometheus_targets_handler))
        .route("/register", post(register_node_handler))
        .route("/heartbeat", post(heartbeat_handler))
        // JSON datasource health check
        .route("/", get(health_check))
        .with_state(app);

    let listener = TcpListener::bind(format!("0.0.0.0:{}", port)).await?;
    info!("Mesh Registry server listening on http://0.0.0.0:{}", port);

    axum::serve(listener, router).await?;

    Ok(())
}

async fn register_node_handler(
    State(app): State<App>,
    Json(registration): Json<NodeRegistration>,
) -> Result<StatusCode, StatusCode> {
    let mut registry = app.registry.lock().await;

    let node_id: u32 = registration
        .id
        .parse()
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let node = Node {
        id: node_id,
        ip: registration.ip.clone(),
        metrics_port: registration.metrics_port,
    };

    registry.add(node);
    info!("Node {} registered", registration.id);

    Ok(StatusCode::OK)
}

async fn heartbeat_handler(
    State(app): State<App>,
    Json(registration): Json<NodeRegistration>,
) -> Result<StatusCode, StatusCode> {
    let mut registry = app.registry.lock().await;

    let node_id: u32 = registration
        .id
        .parse()
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let node = Node {
        id: node_id,
        ip: registration.ip.clone(),
        metrics_port: registration.metrics_port,
    };

    registry.add(node);
    debug!("Heartbeat from node {}", registration.id);

    Ok(StatusCode::OK)
}

/// Background task to periodically clean up stale node registrations.
///
/// This function runs in a background task and periodically calls the registry's
/// purge method to remove nodes that haven't been seen within the TTL period.
/// It runs every 30 seconds and logs when nodes are removed.
async fn cleanup_stale_nodes(registry: Arc<Mutex<Registry>>) {
    let mut interval = tokio::time::interval(Duration::from_secs(30));

    loop {
        interval.tick().await;

        let mut registry = registry.lock().await;
        let before_count = registry.nodes().len();
        registry.purge();
        let after_count = registry.nodes().len();

        if before_count != after_count {
            info!("Purged {} stale nodes", before_count - after_count);
        }
    }
}

async fn graph_handler(State(app): State<App>) -> Result<Json<GraphData>, StatusCode> {
    let nodes = match query_prometheus_for_nodes(&app).await {
        Ok(nodes) => nodes,
        Err(e) => {
            error!("Error querying Prometheus for nodes: {}", e);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    let edges = match query_prometheus_for_edges(&app, &nodes).await {
        Ok(edges) => edges,
        Err(e) => {
            error!("Error querying Prometheus for edges: {}", e);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    Ok(Json(GraphData { nodes, edges }))
}

async fn prometheus_targets_handler(
    State(app): State<App>,
) -> Result<Json<Vec<PrometheusTarget>>, StatusCode> {
    // Use registered nodes from the registry
    let registry = app.registry.lock().await;
    let mut targets = Vec::new();

    for node_entry in registry.nodes() {
        let node_id_str = node_entry.node.id.to_string();

        targets.push(PrometheusTarget {
            targets: vec![format!(
                "{}:{}",
                node_entry.node.ip.clone(),
                node_entry.node.metrics_port
            )],
            labels: PrometheusLabels {
                job: "mesh-node".to_string(),
                node_id: node_id_str,
                node_ip: node_entry.node.ip.clone(),
            },
        });
    }

    debug!("service discovery targets: {}", targets.len());
    Ok(Json(targets))
}

// JSON datasource health check
async fn health_check() -> StatusCode {
    StatusCode::OK
}

async fn query_prometheus_for_nodes(app: &App) -> Result<Vec<GraphNode>> {
    let url = format!("{}/api/v1/query", app.prometheus_endpoint);
    let response = app
        .http_client
        .get(&url)
        .query(&[("query", "mesh_connected_peers_total")])
        .send()
        .await?;

    if !response.status().is_success() {
        error!(
            "Prometheus query failed for connected peers metric: {}",
            response.status()
        );
        return Ok(Vec::new());
    }

    let prom_response: PrometheusResponse = response.json().await?;
    let mut nodes = Vec::new();

    for result in prom_response.data.result {
        let node_id = match result.metric.get("node_id") {
            Some(id) => id.clone(),
            None => {
                error!("Prometheus query result missing node_id: {:?}", result);
                continue;
            }
        };
        let node_ip = match result.metric.get("node_ip") {
            Some(ip) => ip.clone(),
            None => {
                error!("Prometheus query result missing node_ip: {:?}", result);
                continue;
            }
        };
        let node_port = match result.metric.get("node_port") {
            Some(port) => port.parse::<u16>().unwrap_or(0),
            None => 0,
        };

        let connected_peers = match result.value[1].as_str() {
            Some(v) => v.parse::<f64>().unwrap_or(0.0),
            None => 0.0,
        };
        let color = if connected_peers > 0.0 {
            "green"
        } else {
            "red"
        };
        let gn = GraphNode {
            id: node_id.clone(),
            title: format!("Node {}", node_id),
            subtitle: Some(format!("{}:{}", node_ip.clone(), node_port.clone())),
            mainStat: connected_peers,
            secondaryStat: None,
            detail: NodeDetail {
                id: node_id,
                ip: node_ip,
                connected_peers,
            },
            color: Some(color.to_string()),
        };

        nodes.push(gn);
    }

    Ok(nodes)
}

async fn query_prometheus_for_edges(app: &App, nodes: &Vec<GraphNode>) -> Result<Vec<Edge>> {
    let metrics = vec![
        "mesh_peer_rtt_current_milliseconds",
        "mesh_peer_rtt_avg_milliseconds",
    ];

    let mut edge_data: HashMap<String, EdgeDetail> = HashMap::new();

    // Query each metric from Prometheus
    for metric in metrics {
        let url = format!("{}/api/v1/query", app.prometheus_endpoint);
        let response = app
            .http_client
            .get(&url)
            .query(&[("query", metric)])
            .send()
            .await?;

        if !response.status().is_success() {
            error!(
                "Prometheus query failed for metric {}: {}",
                metric,
                response.status()
            );
            continue;
        }

        let prom_response: PrometheusResponse = response.json().await?;

        for result in prom_response.data.result {
            let local_node_id = match result.metric.get("local_node_id") {
                Some(id) => id.clone(),
                None => {
                    error!(
                        "Prometheus query result missing local_node_id: {:?}",
                        result
                    );
                    continue;
                }
            };
            let remote_node_id = match result.metric.get("remote_node_id") {
                Some(id) => id.clone(),
                None => {
                    error!(
                        "Prometheus query result missing remote_node_id: {:?}",
                        result
                    );
                    continue;
                }
            };
            let value = match result.value[1].as_str() {
                Some(v) => match v.parse::<f64>() {
                    Ok(val) => val,
                    Err(_) => {
                        error!("Failed to parse value as f64: {}", v);
                        continue;
                    }
                },
                None => {
                    error!("Prometheus query result missing value: {:?}", result);
                    continue;
                }
            };

            let edge_key = format!("{}_{}", local_node_id, remote_node_id);

            let edge_detail = edge_data.entry(edge_key).or_insert(EdgeDetail {
                rtt_current: None,
                rtt_avg: 0.0,
            });

            match metric {
                "mesh_peer_rtt_current_milliseconds" => edge_detail.rtt_current = Some(value),
                "mesh_peer_rtt_avg_milliseconds" => edge_detail.rtt_avg = value,
                _ => {}
            }
        }
    }

    // Convert to Edge structures
    let mut edges = Vec::new();
    let mut missing: HashSet<String> = HashSet::new();
    for (edge_key, detail) in edge_data {
        let parts: Vec<&str> = edge_key.split('_').collect();
        if parts.len() != 2 {
            debug!("Invalid edge key: {}", edge_key);
            continue;
        }

        let source = parts[0].to_string();
        let target = parts[1].to_string();
        let mut contains = |id: &str| -> bool {
            if missing.contains(id) {
                return false;
            }
            match nodes.iter().any(|n| n.id == id) {
                true => true,
                false => {
                    missing.insert(id.to_string());
                    false
                }
            }
        };

        if !contains(source.as_str()) || !contains(target.as_str()) {
            debug!("Skipping edge with unknown nodes: {} -> {}", source, target);
            continue;
        }

        let main_stat = detail.rtt_current.unwrap_or(detail.rtt_avg);
        let secondary_stat = Some(detail.rtt_avg);

        edges.push(Edge {
            id: edge_key.clone(),
            source,
            target,
            mainStat: main_stat,
            secondaryStat: secondary_stat,
            detail,
        });
    }

    Ok(edges)
}
