use anyhow::{Context, Result};
use axum::{Router, extract::State as AxumState, http::StatusCode, routing::get};
use clap::Parser;
use node::{Connection, Metrics, Node, State};
use noise;
use serde::{Deserialize, Serialize};
use snow::Builder;
use socket2::{Domain, Protocol, Socket, Type};
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::time;
use tokio::{net::UdpSocket, sync::Mutex};
use tracing::{error, info};

/// Command line arguments for the mesh node application.
#[derive(Parser, Debug)]
#[command(name = "mesh-node")]
#[command(about = "A mesh network node")]
struct Args {
    /// Mesh-registry service endpoint for registration
    #[arg(long, default_value = "http://localhost:5000")]
    mesh_registry_endpoint: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct NodeRegistration {
    id: String,
    ip: String,
    metrics_port: u16,
}

/// Pre-shared network key for mesh authentication.
///
/// This 32-byte key is used by all nodes in the mesh network for authentication
/// via the Noise protocol's PSK (Pre-Shared Key) mechanism. Only nodes with
/// the correct key can establish secure connections with each other.
///
/// # Security Note
/// * This is hardcoded just for demonstration purposes.
const NETWORK_KEY: [u8; 32] = [
    0x67, 0xa5, 0x0f, 0x77, 0xa3, 0x51, 0x73, 0xdc, 0xc2, 0xa1, 0x29, 0xf1, 0xd8, 0xb8, 0x52, 0xa5,
    0x35, 0x01, 0x82, 0x09, 0xa0, 0xda, 0x35, 0x7c, 0xe3, 0xf4, 0x75, 0x0e, 0x53, 0x8d, 0xb6, 0x2a,
];

/// Multicast address for peer discovery.
///
/// All nodes broadcast Hello messages to this multicast address to announce
/// their presence. This is a standard IPv4 multicast address in the "All Systems"
/// range that should be routable on most local networks.
const MULTICAST_ADDR: &str = "224.0.0.1";

/// UDP port for multicast communication.
///
/// This port is used for both sending and receiving multicast Hello messages.
/// All nodes in the mesh network must use the same port for discovery to work.
const MULTICAST_PORT: u16 = 9999;

/// Main application entry point.
///
/// This function initializes and starts a mesh network node. It sets up logging,
/// creates the node with all necessary networking components, and spawns the
/// main networking tasks. The application runs until interrupted by Ctrl+C.
///
/// # Architecture
/// The application uses a multi-task architecture:
/// * **Receiver Task**: Handles incoming messages from all peers
/// * **Broadcast Task**: Periodically sends Hello messages for discovery
/// * **Ping Task**: Monitors connection health with established peers
///
/// # Returns
/// Returns `Ok(())` on successful shutdown, or an error if initialization fails.
///
/// # Errors
/// * Node initialization failures (socket creation, key generation, etc.)
/// * Signal handling setup failures
#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    log::init_logging();

    let node = init_node()
        .await
        .context("Failed to initialize node")?;

    // --- Receiver Task ---
    let recv_node = node.clone();
    tokio::spawn(async move { recv_node.receive().await });

    // --- HELLO Broadcast Task ---
    let broadcast_node = node.clone();
    let multicast_addr: SocketAddr = format!("{}:{}", MULTICAST_ADDR, MULTICAST_PORT).parse()?;
    tokio::spawn(async move { broadcast_node.broadcast(multicast_addr).await });

    // --- Ping Task ---
    let ping_node = node.clone();
    tokio::spawn(async move { ping_node.ping_peers().await });

    // --- Peer Health Monitor Task ---
    let health_node = node.clone();
    tokio::spawn(async move { health_node.monitor_peers().await });

    let metrics = Arc::new(Metrics::new()?);

    // --- Metrics Server Task ---
    let metrics_node = node.clone();
    tokio::spawn({
        let metrics_svr = metrics.clone();
        async move {
            if let Err(e) = serve_metrics(metrics_svr, metrics_node.get_port()).await {
                tracing::error!("Metrics server failed: {}", e);
            }
        }
    });

    // --- Metrics Collection Task ---
    let node_metrics = node.clone();
    tokio::spawn(async move { node_metrics.collect_metrics(metrics).await });

    // --- Mesh-Registry Registration Task ---
    let registration_node = node.clone();
    let mesh_registry_endpoint = args.mesh_registry_endpoint.clone();
    let registration_metrics_port = registration_node.get_port();
    tokio::spawn(async move {
        if let Err(e) = heartbeat_task(
            registration_node,
            mesh_registry_endpoint,
            registration_metrics_port,
        )
        .await
        {
            error!("Heartbeat task failed: {}", e);
        }
    });

    tokio::signal::ctrl_c().await?;

    Ok(())
}

async fn heartbeat_task(
    node: Arc<Node>,
    mesh_registry_endpoint: String,
    metrics_port: u16,
) -> Result<()> {
    let client = reqwest::Client::new();
    let mut interval = time::interval(Duration::from_secs(10)); // Heartbeat every 30 seconds

    loop {
        interval.tick().await;

        let registration = NodeRegistration {
            id: node.get_id().to_string(),
            ip: node.get_local_ip(),
            metrics_port,
        };

        // Try to send heartbeat, fallback to registration if needed
        let url = format!("{}/heartbeat", mesh_registry_endpoint);
        info!("Sending heartbeat to mesh-registry at {}", url);
        match client.post(&url).json(&registration).send().await {
            Ok(response) => {
                if response.status().is_success() {
                    info!("Heartbeat sent to mesh-registry");
                } else {
                    error!("Heartbeat failed: {}", response.status());
                }
            }
            Err(e) => {
                error!("Failed to send heartbeat: {}", e);
            }
        }
    }
}

/// Initializes and configures a new mesh node.
///
/// This function performs all the necessary setup to create a fully functional
/// mesh node, including
/// * Generating a unique node ID and cryptographic keys
/// * Setting up multicast and unicast UDP sockets
/// * Initializing state databases for peer and session management
/// * Creating the node with all components properly configured
///
/// # Returns
/// An Arc-wrapped Node instance ready for networking operations.
///
/// # Network Configuration
/// * Multicast socket joins the discovery group and listens on the standard port
/// * Unicast socket binds to an ephemeral port for peer-to-peer communication
/// * Both sockets are configured for optimal performance and reliability
async fn init_node() -> Result<Arc<Node>> {
    let node_id: mesh::NodeId = rand::random();
    info!(node_id = node_id, "Starting node with ID");

    let np = noise::noise_params().context("Failed to create noise parameters")?;
    let builder = Builder::new(np);
    let static_keys = builder.generate_keypair()?;
    info!(
        public_key = hex::encode(&static_keys.public),
        "Our Node ID (public key)",
    );

    let multicast_socket = create_multicast_socket()
        .await
        .context("Failed to create multicast socket")?;
    info!(
        multicast_addr = MULTICAST_ADDR,
        multicast_port = MULTICAST_PORT,
        "Listening for mesh traffic on multicast",
    );
    let unicast_socket = UdpSocket::bind("0.0.0.0:0").await?;
    let local_addr = unicast_socket.local_addr()?;
    let unicast_port = local_addr.port();
    info!(
        unicast_port = unicast_port,
        "Unicast listening on port: {}", unicast_port
    );

    let local_ip = get_local_ip().await?;
    info!(local_ip = local_ip, "Detected local IP address");

    let link_state_db: node::LinkStateDb = Arc::new(Mutex::new(HashMap::new()));
    let session_state_db: node::SessionDb = Arc::new(Mutex::new(HashMap::new()));
    let ping_state_db: node::PingDb = Arc::new(Mutex::new(HashMap::new()));
    let state = State::new(link_state_db, session_state_db, ping_state_db);

    let multicast_socket = Arc::new(multicast_socket);
    let unicast_socket = Arc::new(unicast_socket);
    let connection = Connection::new(
        unicast_port,
        unicast_socket.clone(),
        multicast_socket.clone(),
    );

    Ok(Arc::new(Node::new(
        node_id,
        NETWORK_KEY,
        static_keys,
        connection,
        state,
        local_ip,
    )))
}

async fn get_local_ip() -> Result<String> {
    let socket = UdpSocket::bind("0.0.0.0:0").await?;
    socket.connect("8.8.8.8:80").await?;
    let local_addr = socket.local_addr()?;
    Ok(local_addr.ip().to_string())
}

/// Creates and configures a UDP socket for multicast communication.
///
/// This function sets up a UDP socket with the necessary options for reliable
/// multicast communication in the mesh network. It configures socket reuse,
/// joins the multicast group, and sets up non-blocking operation.
///
/// # Returns
/// A configured UdpSocket ready for multicast operations.
///
/// # Socket Configuration
/// * **SO_REUSEADDR**: Allows multiple processes to bind to the same address
/// * **SO_REUSEPORT**: Enables load balancing across multiple sockets
/// * **Multicast Join**: Joins the discovery multicast group
/// * **Non-blocking**: Configured for async operation with tokio
async fn create_multicast_socket() -> Result<UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))
        .context("Failed to create UDP socket")?;

    socket.set_reuse_address(true)?;
    socket.set_reuse_port(true)?;

    let addr: SocketAddr = format!("0.0.0.0:{}", MULTICAST_PORT).parse()?;
    socket.bind(&addr.into())?;

    let multicast_addr: Ipv4Addr = MULTICAST_ADDR.parse()?;
    let interface = Ipv4Addr::UNSPECIFIED;
    socket.join_multicast_v4(&multicast_addr, &interface)?;

    socket.set_nonblocking(true)?;
    Ok(UdpSocket::from_std(socket.into())?)
}

/// Starts a simple HTTP server to expose metrics on the /metrics endpoint.
pub async fn serve_metrics(metrics: Arc<Metrics>, port: u16) -> Result<()> {
    let app = Router::new()
        .route("/metrics", get(metrics_handler))
        .with_state(metrics);

    let listener = TcpListener::bind(format!("0.0.0.0:{}", port)).await?;
    info!(
        "Metrics server listening on http://0.0.0.0:{}/metrics",
        port
    );

    axum::serve(listener, app).await?;
    Ok(())
}

async fn metrics_handler(
    AxumState(metrics): AxumState<Arc<Metrics>>,
) -> Result<String, StatusCode> {
    metrics
        .render()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}
