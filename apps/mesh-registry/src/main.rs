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
use std::collections::HashMap;
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

// Node registration structures
#[derive(Serialize, Deserialize, Debug)]
pub struct NodeRegistration {
    pub id: String,
    pub ip: String,
    pub metrics_port: u16,
}

// Data structures for Grafana node graph panel
#[derive(Serialize, Deserialize, Debug)]
pub struct GraphNode {
    pub id: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub mainStat: f64,
    pub secondaryStat: Option<f64>,
    pub detail: NodeDetail,
    pub color: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct NodeDetail {
    pub ip: String,
    pub id: String,
    pub connected_peers: f64,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Edge {
    pub id: String,
    pub source: String,
    pub target: String,
    pub mainStat: f64,
    pub secondaryStat: Option<f64>,
    pub detail: EdgeDetail,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct EdgeDetail {
    pub rtt_current: Option<f64>,
    pub rtt_avg: f64,
}

// Prometheus service discovery structures
#[derive(Serialize, Deserialize, Debug)]
pub struct PrometheusTarget {
    pub targets: Vec<String>,
    pub labels: PrometheusLabels,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct PrometheusLabels {
    pub job: String,
    pub node_id: String,
    pub node_ip: String,
}

// Prometheus query response structures
#[derive(Deserialize, Debug)]
struct PrometheusResponse {
    data: PrometheusData,
}

#[derive(Deserialize, Debug)]
struct PrometheusData {
    result: Vec<PrometheusResult>,
}

#[derive(Deserialize, Debug)]
struct PrometheusResult {
    metric: HashMap<String, String>,
    value: [serde_json::Value; 2],
}

// Application state
#[derive(Clone)]
pub struct App {
    pub prometheus_endpoint: String,
    pub http_client: reqwest::Client,
    pub registry: Arc<Mutex<Registry>>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    log::init_logging();

    let http_client = reqwest::Client::new();
    let registry = Arc::new(Mutex::new(Registry::new(Duration::from_secs(120)))); // 2 minute TTL

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

async fn serve(app: App, port: u16) -> Result<()> {
    let router = Router::new()
        .route("/graph/nodes", get(nodes_handler))
        .route("/graph/edges", get(edges_handler))
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

async fn nodes_handler(State(app): State<App>) -> Result<Json<Vec<GraphNode>>, StatusCode> {
    match query_prometheus_for_nodes(&app).await {
        Ok(nodes) => Ok(Json(nodes)),
        Err(e) => {
            error!("Error querying Prometheus for nodes: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn edges_handler(State(app): State<App>) -> Result<Json<Vec<Edge>>, StatusCode> {
    match query_prometheus_for_edges(&app).await {
        Ok(edges) => Ok(Json(edges)),
        Err(e) => {
            error!("Error querying Prometheus for edges: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn prometheus_targets_handler(
    State(app): State<App>,
) -> Result<Json<Vec<PrometheusTarget>>, StatusCode> {
    // Use registered nodes from the registry
    let registry = app.registry.lock().await;
    let mut targets = Vec::new();

    for node_entry in registry.nodes() {
        let node_id_str = node_entry.node.id.to_string();
        let metrics_port = node_entry.node.metrics_port;

        targets.push(PrometheusTarget {
            targets: vec![format!("{}:{}", node_entry.node.ip, node_entry.node.metrics_port)],
            labels: PrometheusLabels {
                job: "mesh-node".to_string(),
                node_id: node_id_str,
                node_ip: node_entry.node.ip.clone(),
            },
        });
    }

    info!("service discovery targets: {}", targets.len());
    Ok(Json(targets))
}

// JSON datasource health check
async fn health_check() -> StatusCode {
    StatusCode::OK
}

async fn query_prometheus_for_targets(app: &App) -> Result<Vec<PrometheusTarget>> {
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
    let mut targets = Vec::new();

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
        let metrics_port = match result.metric.get("node_metric_port") {
            Some(port) => port.clone(),
            None => {
                error!(
                    "Prometheus query result missing node_metric_port: {:?}",
                    result
                );
                continue;
            }
        };

        targets.push(PrometheusTarget {
            targets: vec![format!("{}:{}", node_ip, metrics_port)],
            labels: PrometheusLabels {
                job: "mesh-node".to_string(),
                node_id,
                node_ip,
            },
        });
    }

    info!("service discovery targets: {}", targets.len());

    Ok(targets)
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
            subtitle: Some(node_ip.clone()),
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

async fn query_prometheus_for_edges(app: &App) -> Result<Vec<Edge>> {
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
    for (edge_key, detail) in edge_data {
        let parts: Vec<&str> = edge_key.split('_').collect();
        if parts.len() != 2 {
            debug!("Invalid edge key: {}", edge_key);
            continue;
        }

        let source = parts[0].to_string();
        let target = parts[1].to_string();
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
