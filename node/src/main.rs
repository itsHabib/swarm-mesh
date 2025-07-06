use anyhow::{Context, Result, anyhow};
use bytemuck::{Pod, Zeroable};
use serde::{Deserialize, Serialize};
use snow::params::NoiseParams;
use snow::{Builder, HandshakeState, TransportState};
use socket2::{Domain, Protocol, Socket, Type};
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::{net::UdpSocket, sync::Mutex, time, time::Instant as TokioInstant};

const MESH_PORT: u16 = 9999;
const HELLO_INTERVAL_MS: u64 = 5000;

type NodeId = u32;

const NETWORK_KEY: [u8; 32] = [
    0x67, 0xa5, 0x0f, 0x77, 0xa3, 0x51, 0x73, 0xdc, 0xc2, 0xa1, 0x29, 0xf1, 0xd8, 0xb8, 0x52, 0xa5,
    0x35, 0x01, 0x82, 0x09, 0xa0, 0xda, 0x35, 0x7c, 0xe3, 0xf4, 0x75, 0x0e, 0x53, 0x8d, 0xb6, 0x2a,
];

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct MeshHeader {
    version: u8,
    msg_type: u8,
    hop_count: u16,
    hop_max: u16,
    _pad: u16,
    src_id: NodeId,
    dst_id: NodeId,
}

#[derive(Serialize, Deserialize, Clone)]
struct HelloPayload {
    node_id: NodeId,
    unicast_port: u16,
}

#[derive(Serialize, Deserialize, Clone)]
struct HandshakePayload {
    node_id: NodeId,
    msg: Vec<u8>,
    unicast_port: u16,
}

#[derive(Serialize, Deserialize, Clone)]
enum MeshMessage {
    Hello(HelloPayload),
    Handshake(HandshakePayload),
    EncryptedData(Vec<u8>),
}

struct PeerInfo {
    last_seen: Instant,
    addr: SocketAddr,
}

type LinkStateDb = Arc<Mutex<HashMap<NodeId, PeerInfo>>>;
type SessionDb = Arc<Mutex<HashMap<NodeId, PeerSession>>>;

enum PeerState {
    HandshakeInProgress { state: snow::HandshakeState },
    Established { cipher: snow::TransportState },
}

struct PeerSession {
    noise: NoiseState,
}

#[derive(Debug)]
enum NoiseState {
    ExpectingInitiator,
    Handshaking(HandshakeState),
    Transport(TransportState),
    Transitioning,
}

#[derive(Debug, Error)]
enum MeshError {
    #[error("already connected to peer")]
    AlreadyConnected,
    #[error("becoming responder")]
    BecomingResponder,
    #[error("error {0}")]
    Other(#[from] anyhow::Error),
}

#[tokio::main]
async fn main() -> Result<()> {
    let node_id: NodeId = rand::random();
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

    let link_state_db: LinkStateDb = Arc::new(Mutex::new(HashMap::new()));
    let session_state_db: SessionDb = Arc::new(Mutex::new(HashMap::new()));
    let multicast_socket = Arc::new(multicast_socket);
    let unicast_socket = Arc::new(unicast_socket);

    // --- Receiver Task ---
    let recv_multicast_socket = multicast_socket.clone();
    let recv_unicast_socket = unicast_socket.clone();
    let link_db_recv = link_state_db.clone();
    let session_db_recv = session_state_db.clone();

    tokio::spawn(async move {
        receiver_task(
            node_id,
            unicast_port,
            recv_multicast_socket,
            recv_unicast_socket,
            link_db_recv,
            session_db_recv,
            static_keys,
        )
        .await
    });

    // --- HELLO Broadcast Task ---
    let broadcast_socket = multicast_socket.clone();
    tokio::spawn(async move {
        if let Err(e) = broadcast_task(broadcast_socket, node_id, unicast_port).await {
            eprintln!("Error in HELLO broadcast task: {:#}", e);
        }
    });

    tokio::signal::ctrl_c().await?;

    Ok(())
}

async fn broadcast_task(socket: Arc<UdpSocket>, node_id: NodeId, udp_port: u16) -> Result<()> {
    let multicast_addr: SocketAddr = format!("{}:{}", MULTICAST_ADDR, MULTICAST_PORT).parse()?;
    let mut interval = time::interval_at(
        TokioInstant::now() + Duration::from_millis(HELLO_INTERVAL_MS),
        Duration::from_millis(HELLO_INTERVAL_MS),
    );

    loop {
        interval.tick().await;

        let hello_msg = MeshMessage::Hello(HelloPayload {
            node_id,
            unicast_port: udp_port,
        });

        send_message(hello_msg.clone(), multicast_addr, &socket).await?;
    }
}

const MULTICAST_ADDR: &str = "224.0.0.1";
const MULTICAST_PORT: u16 = 9999;

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

async fn receiver_task(
    node_id: NodeId,
    unicast_port: u16,
    multicast_socket: Arc<UdpSocket>,
    unicast_socket: Arc<UdpSocket>,
    link_db_recv: LinkStateDb,
    session_db_recv: SessionDb,
    static_keys: snow::Keypair,
) -> ! {
    loop {
        let mut mcast_buf = [0u8; 1500];
        let mut ucast_buf = [0u8; 1500];
        let (len, src_addr, msg) = tokio::select! {
            Ok((len, src_addr)) = multicast_socket.recv_from(&mut mcast_buf) => {
                match bincode::deserialize::<MeshMessage>(&mcast_buf[0..len]) {
                    Ok(msg @ MeshMessage::Hello(_)) => (len, src_addr, msg),
                    _ => {
                        println!("Received non-HELLO message from {} on multicast, ignoring", src_addr);
                        continue; // Skip non-HELLO messages
                    }
            }
        },
            Ok((len, src_addr)) = unicast_socket.recv_from(&mut ucast_buf) => {
           match bincode::deserialize::<MeshMessage>(&ucast_buf[0..len]) {
                    Ok(msg) => (len, src_addr, msg),
                    Err(_) => continue,
                }
                }
            };

        match msg {
            MeshMessage::Hello(payload) => {
                if payload.node_id == node_id {
                    // Ignore our own messages
                    continue;
                }

                if session_db_recv.lock().await.contains_key(&payload.node_id) {
                    link_db_recv
                        .lock()
                        .await
                        .entry(payload.node_id)
                        .and_modify(|p| {
                            p.last_seen = Instant::now();
                            p.addr = src_addr;
                        })
                        .or_insert(PeerInfo {
                            last_seen: Instant::now(),
                            addr: src_addr,
                        });
                    continue;
                }

                match handle_hello(
                    &payload,
                    src_addr,
                    session_db_recv.clone(),
                    &static_keys,
                    node_id,
                    unicast_port,
                )
                .await
                {
                    Ok(Some(msg)) => {
                        let mut unicast_addr = src_addr;
                        unicast_addr.set_port(payload.unicast_port);
                        println!("Sending start handshake response to {}", unicast_addr);

                        if let Err(e) = send_message(msg, unicast_addr, &unicast_socket).await {
                            eprintln!("Error sending handshake response: {}", e);
                        }
                    }
                    Ok(None) => continue,
                    Err(MeshError::AlreadyConnected) => {
                        continue; // Skip sending a message
                    }
                    Err(e) => {
                        eprintln!("Error handling HELLO: {}", e);
                    }
                };
            }
            MeshMessage::Handshake(payload) => {
                if payload.node_id == node_id {
                    // Ignore our own messages
                    continue;
                }

                let unicast_port = payload.unicast_port.clone();
                let msg = match handle_handshake(
                    node_id,
                    payload,
                    session_db_recv.clone(),
                    src_addr,
                    &static_keys,
                )
                .await {
                    Ok(msg) => msg,
                    Err(e) => {
                        eprintln!("Error handling handshake: {}", e);
                        continue; // Skip sending a message
                    }
                };

                let mut unicast_addr = src_addr;
                unicast_addr.set_port(unicast_port);
                println!("Sending handshake response to {}", unicast_addr);

                if let Err(e) = send_message(msg, unicast_addr, &unicast_socket).await {
                    eprintln!("Error sending handshake response: {}", e);
                }
            }
            _ => {
                println!("Received unknown message from {}", src_addr);
            }
        }
    }
}

async fn handle_hello(
    payload: &HelloPayload,
    src_addr: SocketAddr,
    session_db_recv: SessionDb,
    static_keys: &snow::Keypair,
    id: NodeId,
    udp_port: u16,
) -> Result<Option<MeshMessage>, MeshError> {
    println!("New Peer {}, @{}", payload.node_id, src_addr);

    if id < payload.node_id {
        println!("our node id {} is less than peers, becoming responder", id);
        let mut db = session_db_recv.lock().await;
        db.insert(
            payload.node_id,
            PeerSession {
                noise: NoiseState::ExpectingInitiator,
            },
        );
        println!(
            "session inserted with EXPECTINGINITIATOR for node {}",
            payload.node_id
        );
        return Ok(None);
    }

    println!(
        "our node id {} is greater than peers, starting handshake",
        id
    );
    let (state, msg) = start_handshake(&static_keys).context("failed to start handshake")?;

    let session_state: NoiseState = NoiseState::Handshaking(state);
    let mut db = session_db_recv.lock().await;
    db.insert(
        payload.node_id,
        PeerSession {
            noise: session_state,
        },
    );
    println!(
        "session inserted with HANDSHAKING for node {}",
        payload.node_id
    );

    let pl = HandshakePayload {
        node_id: id,
        msg,
        unicast_port: udp_port,
    };

    Ok(Some(MeshMessage::Handshake(pl)))
}

async fn handle_handshake(
    id: NodeId,
    HandshakePayload {
        node_id,
        msg,
        unicast_port: udp_port,
    }: HandshakePayload,
    session_db_recv: SessionDb,
    src_addr: SocketAddr,
    static_keys: &snow::Keypair,
) -> Result<MeshMessage> {
    println!(
        "Received HANDSHAKE from {} @{}, msg len: {}",
        node_id,
        src_addr,
        msg.len()
    );

    let mut db = session_db_recv.lock().await;
    let peer_session = match db.get_mut(&node_id) {
        Some(session) => session,
        None => {
            if id < node_id {
                println!("Creating responder session from unknown peer {}", node_id);
                db.insert(
                    node_id,
                    PeerSession {
                        noise: NoiseState::ExpectingInitiator,
                    },
                );
                db.get_mut(&node_id).unwrap()
            } else {
                return Err(anyhow!(
                    "Received unsolicited handshake from unknown peer {}",
                    node_id
                ));
            }
        }
    };

    println!("Found session for node {}", node_id);

    match std::mem::replace(&mut peer_session.noise, NoiseState::Transitioning) {
        NoiseState::ExpectingInitiator => {
            println!(
                "Node {}: acting as responder for handshake(EXPECTINGINITIATIOR)",
                id
            );
            let mut responder_state = build_responder(&static_keys)?;

            let mut read_buf = [0u8; 1024];
            responder_state.read_message(&msg, &mut read_buf)?;

            let mut write_buf = [0u8; 1024];
            let len = responder_state.write_message(&[], &mut write_buf)?;

            peer_session.noise = NoiseState::Handshaking(responder_state);

            Ok(MeshMessage::Handshake(HandshakePayload {
                node_id: id,
                msg: write_buf[..len].to_vec(),
                unicast_port: udp_port,
            }))
        }
        NoiseState::Handshaking(mut state) => {
            println!(
                "Node {}: continuing handshake with existing state(HANDSHAKING)",
                id
            );
            let mut read_buf = [0u8; 1024];
            if let Err(e) = state.read_message(&msg, &mut read_buf) {
                eprintln!("Error reading message from {}: {}", src_addr, e);
                return Err(anyhow!("unable to read msg"));
            }

            let mut write_buf = [0u8; 1024];
            let len = state
                .write_message(&[], &mut write_buf)
                .context("error writing message")?;

            if state.is_handshake_finished() {
                println!("Hanshake with {} completed, session is now secure", node_id);

                let transport_state = state
                    .into_transport_mode()
                    .context("error converting to transport mode")?;

                peer_session.noise = NoiseState::Transport(transport_state);
            } else {
                println!(
                    "Handshake with {} not yet complete, still in handshaking state",
                    node_id
                );
                peer_session.noise = NoiseState::Handshaking(state);
            }

            Ok(MeshMessage::Handshake(HandshakePayload {
                node_id: id,
                msg: write_buf[..len].to_vec(),
                unicast_port: udp_port,
            }))
        }
        other_state => {
            let err = anyhow!("unexpected noise state: {:?}", other_state);
            peer_session.noise = other_state;
            Err(err)
        }
    }
}

async fn send_message(msg: MeshMessage, addr: SocketAddr, socket: &Arc<UdpSocket>) -> Result<()> {
    socket
        .send_to(
            &bincode::serialize(&msg).context("Error serializing message")?,
            addr,
        )
        .await
        .context("Error sending message")?;

    Ok(())
}

// initiator
fn start_handshake(static_keys: &snow::Keypair) -> Result<(HandshakeState, Vec<u8>)> {
    let noise_params = noise_params().unwrap();
    let mut initiator_state = Builder::new(noise_params)
        .local_private_key(&static_keys.private)
        .psk(3, &NETWORK_KEY)
        .build_initiator()?;

    let mut buf = [0u8; 1024];
    // -> e
    let len = initiator_state.write_message(&[], &mut buf)?;
    let first_message = buf[..len].to_vec();

    Ok((initiator_state, first_message))
}

fn build_responder(static_keys: &snow::Keypair) -> Result<HandshakeState> {
    Builder::new(noise_params()?)
        .local_private_key(&static_keys.private)
        .psk(3, &NETWORK_KEY)
        .build_responder()
        .context("Failed to build responder state")
}

fn noise_params() -> Result<NoiseParams> {
    "Noise_XXpsk3_25519_ChaChaPoly_BLAKE2s"
        .parse::<NoiseParams>()
        .context("Invalid noise params")
}
