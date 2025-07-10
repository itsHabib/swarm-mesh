use anyhow::{Context, Result};
use node::{Connection, Node, State};
use noise;
use snow::Builder;
use socket2::{Domain, Protocol, Socket, Type};
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use tokio::{net::UdpSocket, sync::Mutex};
use tracing::info;
use tracing_log::LogTracer;
use tracing_subscriber::{EnvFilter, fmt};

/// Pre-shared network key for mesh authentication.
/// 
/// This 32-byte key is used by all nodes in the mesh network for authentication
/// via the Noise protocol's PSK (Pre-Shared Key) mechanism. Only nodes with
/// the correct key can establish secure connections with each other.
/// 
/// # Security Note
/// In a production deployment, this key should be:
/// * Generated randomly and securely distributed
/// * Stored securely (e.g., in a key management system)
/// * Rotated periodically for enhanced security
/// * Never hardcoded in source code
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
    init_logging();
    let node = init_node().await.context("Failed to initialize node")?;

    // --- Receiver Task ---
    let recv_node = node.clone();
    tokio::spawn(async move { recv_node.receive().await });

    // --- HELLO Broadcast Task ---
    let broadcast_node = node.clone();
    let multicast_addr: SocketAddr = format!("{}:{}", MULTICAST_ADDR, MULTICAST_PORT).parse()?;
    tokio::spawn(async move { broadcast_node.broadcast(multicast_addr).await });

    // -- Ping Task ---
    let ping_node = node.clone();
    tokio::spawn(async move { ping_node.ping_peers().await });

    tokio::signal::ctrl_c().await?;

    Ok(())
}

/// Initializes and configures a new mesh node.
/// 
/// This function performs all the necessary setup to create a fully functional
/// mesh node, including:
/// * Generating a unique node ID and cryptographic keys
/// * Setting up multicast and unicast UDP sockets
/// * Initializing state databases for peer and session management
/// * Creating the node with all components properly configured
/// 
/// # Returns
/// An Arc-wrapped Node instance ready for networking operations.
/// 
/// # Errors
/// * Random number generation failures
/// * Noise protocol initialization errors
/// * Socket creation or configuration failures
/// * Network binding errors
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
    let unicast_port = unicast_socket.local_addr()?.port();
    info!(
        unicast_port = unicast_port,
        "Unicast listening on port: {}", unicast_port
    );

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
    )))
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
/// # Errors
/// * Socket creation failures
/// * Socket option configuration errors
/// * Multicast group join failures
/// * Address binding errors
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

/// Initializes the logging subsystem for the application.
/// 
/// This function sets up structured logging using the tracing ecosystem,
/// configured for JSON output with detailed span information. The logging
/// level can be controlled via the RUST_LOG environment variable.
/// 
/// # Configuration
/// * **Format**: JSON for machine-readable logs
/// * **Spans**: Includes current span context in log entries
/// * **Events**: Logs span close events for timing information
/// * **Filter**: Respects RUST_LOG environment variable
/// 
/// # Panics
/// Panics if the logging system cannot be initialized (e.g., if already initialized).
/// 
/// # Usage
/// Set RUST_LOG environment variable to control logging:
/// * `RUST_LOG=debug` - Verbose debugging information
/// * `RUST_LOG=info` - General information messages
/// * `RUST_LOG=warn` - Warning messages only
/// * `RUST_LOG=error` - Error messages only
fn init_logging() {
    LogTracer::init().expect("Failed to set logger");

    let subscriber = fmt::Subscriber::builder()
        .with_env_filter(EnvFilter::from_default_env())
        .json()
        .with_current_span(true)
        .with_span_events(fmt::format::FmtSpan::CLOSE) // optional
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("Failed to set global subscriber");
}
