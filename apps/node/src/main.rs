use anyhow::{Context, Result};
use node::{Connection, Node, State};
use snow::Builder;
use socket2::{Domain, Protocol, Socket, Type};
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use tokio::{net::UdpSocket, sync::Mutex};
use tracing;
use tracing_log::LogTracer;
use tracing_subscriber::{fmt, EnvFilter};


const NETWORK_KEY: [u8; 32] = [
    0x67, 0xa5, 0x0f, 0x77, 0xa3, 0x51, 0x73, 0xdc, 0xc2, 0xa1, 0x29, 0xf1, 0xd8, 0xb8, 0x52, 0xa5,
    0x35, 0x01, 0x82, 0x09, 0xa0, 0xda, 0x35, 0x7c, 0xe3, 0xf4, 0x75, 0x0e, 0x53, 0x8d, 0xb6, 0x2a,
];
const MULTICAST_ADDR: &str = "224.0.0.1";
const MULTICAST_PORT: u16 = 9999;


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

    tokio::signal::ctrl_c().await?;

    Ok(())
}

async fn init_node() -> Result<Arc<Node>> {
    let node_id: mesh::NodeId = rand::random();
    println!("Starting node with ID: {}", node_id);

    let np = "Noise_XX_25519_ChaChaPoly_BLAKE2s".parse()?;
    let builder = Builder::new(np);
    let static_keys = builder.generate_keypair()?;
    println!(
        "Our Node ID (public key): {}",
        hex::encode(&static_keys.public)
    );

    let multicast_socket = create_multicast_socket()
        .await
        .context("Failed to create multicast socket")?;
    println!(
        "Listening for mesh traffic on multicast {}:{}",
        MULTICAST_ADDR, MULTICAST_PORT
    );
    let unicast_socket = UdpSocket::bind("0.0.0.0:0").await?;
    let unicast_port = unicast_socket.local_addr()?.port();
    println!("Unicast listening on port: {}", unicast_port);

    let link_state_db: node::LinkStateDb = Arc::new(Mutex::new(HashMap::new()));
    let session_state_db: node::SessionDb = Arc::new(Mutex::new(HashMap::new()));
    let multicast_socket = Arc::new(multicast_socket);
    let unicast_socket = Arc::new(unicast_socket);

    let state = State::new(link_state_db, session_state_db);
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