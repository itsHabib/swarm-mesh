use crate::metrics::Metrics;
use crate::{
    connection::Connection,
    state::{PingDb, SessionDb, State},
};
use anyhow::{Context, Result, anyhow};
use futures::future::join_all;
use mesh::{
    Error as MeshError, HandshakePayload, HelloPayload, Message, NodeId, NoiseState, PeerInfo,
    PeerSession, PingPongPayload, RttStats,
};
use noise;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::time;
use tracing::{debug, error, info};

/// Interval between Hello message broadcasts in milliseconds.
///
/// Hello messages are sent periodically to announce the node's presence
/// to other nodes in the mesh network. A 15-second interval provides a
/// good balance between network discovery responsiveness and bandwidth usage.
const HELLO_INTERVAL_MS: u64 = 15000;

/// Interval between ping messages in milliseconds.
///
/// Ping messages are sent to established peers to monitor connection health
/// and measure round-trip times. An 8-second interval ensures timely detection
/// of connection issues while avoiding excessive network traffic.
const PING_INTERVAL_MS: u64 = 8000;

/// Core mesh network node implementation.
///
/// The Node struct represents a single participant in the mesh network. It handles
/// all aspects of mesh networking including peer discovery, secure session establishment,
/// connection monitoring, and message routing. Each node operates independently and
/// can communicate with multiple peers simultaneously.
///
/// # Key Responsibilities
/// * **Peer Discovery**: Broadcasting Hello messages and discovering other nodes
/// * **Security**: Establishing encrypted sessions using the Noise protocol
/// * **Connection Management**: Tracking peer states and connection health
/// * **Message Routing**: Sending and receiving various message types
/// * **Network Monitoring**: Measuring RTT and detecting connection failures
///
/// # Architecture
/// The node uses an event-driven architecture with separate async tasks for:
/// * Message reception and processing
/// * Periodic Hello message broadcasting
/// * Ping/pong health monitoring
///
/// # Thread Safety
/// The Node struct is designed to be shared across multiple async tasks using
/// Arc<Node>. All internal state is protected by appropriate synchronization
/// primitives.
pub struct Node {
    /// Unique identifier for this node in the mesh network
    id: u32,
    /// Pre-shared key for network-level authentication
    network_key: [u8; 32],
    /// Long-term cryptographic keypair for Noise protocol authentication
    static_keys: snow::Keypair,
    /// Network connection management (sockets and ports)
    connection: Connection,
    /// Shared state databases for peers, sessions, and ping tracking
    state: State,
    /// Port where this node's metrics server is listening
    metrics_port: u16,
    /// IP of the node
    ip: String,
}

impl Node {
    /// Creates a new Node instance with the specified configuration.
    ///
    /// This constructor initializes a mesh node with all the necessary components
    /// for participating in the mesh network. The node will be ready to start
    /// networking operations after creation.
    ///
    /// # Arguments
    /// * `id` - Unique 32-bit identifier for this node
    /// * `network_key` - 32-byte pre-shared key for network authentication
    /// * `static_keys` - Noise protocol keypair for cryptographic authentication
    /// * `connection` - Network connection management with configured sockets
    /// * `state` - State management with initialized databases
    ///
    /// # Returns
    /// A new Node instance ready for mesh networking operations
    ///
    /// # Usage
    /// This constructor is typically called during application startup after
    /// all the networking components have been properly initialized and configured.
    pub fn new(
        id: u32,
        network_key: [u8; 32],
        static_keys: snow::Keypair,
        connection: Connection,
        state: State,
        ip: String,
        metrics_port: u16,
    ) -> Self {
        Node {
            id,
            network_key,
            static_keys,
            connection,
            state,
            ip,
            metrics_port,
        }
    }

    /// Main message reception loop for the node.
    ///
    /// This method runs an infinite loop that listens for incoming messages on both
    /// multicast and unicast sockets. It processes different message types and handles
    /// the mesh networking protocol logic. This is the core message processing engine
    /// of the node.
    ///
    /// # Message Processing
    /// * **Multicast Messages**: Only Hello messages are accepted from multicast
    /// * **Unicast Messages**: All message types (Handshake, Ping, Pong, EncryptedData)
    /// * **Self-filtering**: Ignores messages from the node's own ID
    /// * **Protocol Handling**: Routes messages to appropriate handlers
    ///
    /// # Message Types Handled
    /// * `Hello` - Peer discovery and session initiation
    /// * `Handshake` - Noise protocol handshake progression
    /// * `Ping` - Connection health requests
    /// * `Pong` - Connection health responses
    /// * `EncryptedData` - Application data (future use)
    ///
    /// # Error Handling
    /// The method logs errors but continues processing to maintain network resilience.
    /// Individual message processing failures don't terminate the receive loop.
    ///
    /// # Never Returns
    /// This method runs indefinitely until the process is terminated. It's designed
    /// to be spawned as a long-running async task.
    pub async fn receive(&self) -> ! {
        let session_db_recv = self.state.session_db.clone();
        let link_db_recv = self.state.link_db.clone();

        loop {
            let mut mcast_buf = [0u8; 1500];
            let mut ucast_buf = [0u8; 1500];

            let (len, src_addr, msg) = tokio::select! {
                Ok((len, src_addr)) = self.connection.multicast_socket.recv_from(&mut mcast_buf) => {
                    match bincode::deserialize::<Message>(&mcast_buf[0..len]) {
                        Ok(msg @ Message::Hello(_)) => (len, src_addr, msg),
                        _ => {
                            info!(addr = src_addr.to_string(), "received non-HELLO message from on multicast, ignoring");
                            continue; // Skip non-HELLO messages
                        }
                    }
                },
                Ok((len, src_addr)) = self.connection.unicast_socket.recv_from(&mut ucast_buf) => {
                    match bincode::deserialize::<Message>(&ucast_buf[0..len]) {
                        Ok(msg) => (len, src_addr, msg),
                        Err(_) => continue,
                    }
                }
            };

            match msg {
                Message::Ping(payload) => {
                    if payload.node_id == self.id {
                        continue;
                    }

                    match self.handle_ping(&payload).await {
                        Ok(Some((unicast_port, pong))) => {
                            if let Err(e) = self
                                .send_unicast_message(pong, unicast_port, src_addr)
                                .await
                            {
                                error!(error = %e, "error sending pong response");
                            }
                        }
                        Ok(None) => {
                            continue;
                        }
                        Err(e) => {
                            error!(error = %e, "error handling ping");
                        }
                    }
                }
                Message::Pong(payload) => {
                    if payload.node_id == self.id {
                        continue;
                    }

                    if let Err(e) = self.handle_pong(&payload).await {
                        error!(error = %e, "error handling pong");
                    }
                }
                Message::Hello(payload) => {
                    if payload.node_id == self.id {
                        // ignore our own messages
                        continue;
                    }

                    if session_db_recv.lock().await.contains_key(&payload.node_id) {
                        link_db_recv
                            .lock()
                            .await
                            .entry(payload.node_id)
                            .and_modify(|p| {
                                p.update(src_addr, payload.unicast_port);
                            })
                            .or_insert(PeerInfo::new(src_addr, payload.unicast_port));
                        continue;
                    }

                    match self
                        .handle_hello(&payload, src_addr, session_db_recv.clone())
                        .await
                    {
                        Ok(Some(msg)) => {
                            let mut unicast_addr = src_addr;
                            unicast_addr.set_port(payload.unicast_port);
                            info!(
                                unicast_addr = unicast_addr.to_string(),
                                "Sending start handshake response"
                            );

                            info!("About to call send_unicast_message"); // Add this line

                            if let Err(e) = self
                                .send_unicast_message(msg, payload.unicast_port, src_addr)
                                .await
                            {
                                error!(error = %e, "Error sending handshake response");
                            }

                            info!("after send_unicast_message"); // Add this line
                        }
                        Ok(None) => continue,
                        Err(MeshError::AlreadyConnected) => {
                            continue; // Skip sending a message
                        }
                        Err(e) => {
                            error!(error = %e, "Error handling HELLO");
                        }
                    };
                }
                Message::Handshake(payload) => {
                    if payload.node_id == self.id {
                        // Ignore our own messages
                        continue;
                    }

                    let unicast_port = payload.unicast_port.clone();
                    let msg = match self
                        .handle_handshake(payload, session_db_recv.clone(), src_addr)
                        .await
                    {
                        Ok(Some(msg)) => msg,
                        Ok(None) => continue,
                        Err(e) => {
                            error!("Error handling handshake: {}", e);
                            continue; // Skip sending a message
                        }
                    };

                    let mut unicast_addr = src_addr;
                    unicast_addr.set_port(unicast_port);
                    info!(unicast_addr = %unicast_addr, "sending handshake response");

                    if let Err(e) = self.send_unicast_message(msg, unicast_port, src_addr).await {
                        error!(error = %e, "error sending handshake response");
                    }
                }
                _ => {
                    info!(src_addr = %src_addr, "received unknown message from");
                }
            }
        }
    }

    /// Periodic Hello message broadcasting loop.
    ///
    /// This method runs an infinite loop that broadcasts Hello messages at regular
    /// intervals to announce the node's presence to other nodes in the mesh network.
    /// Hello messages are essential for peer discovery and network formation.
    ///
    /// # Arguments
    /// * `addr` - The multicast address to broadcast Hello messages to
    ///
    /// # Broadcast Schedule
    /// * **Initial Delay**: Waits one full interval before first broadcast
    /// * **Interval**: Broadcasts every `HELLO_INTERVAL_MS` milliseconds (15 seconds)
    /// * **Persistence**: Continues broadcasting for the lifetime of the node
    ///
    /// # Message Content
    /// Each Hello message contains:
    /// * Node's unique identifier
    /// * Unicast port for direct communication
    ///
    /// # Error Handling
    /// Broadcast failures are logged but don't terminate the loop, ensuring
    /// the node continues attempting to maintain network presence.
    ///
    /// # Never Returns
    /// This method runs indefinitely until the process is terminated. It's designed
    /// to be spawned as a long-running async task.
    pub async fn broadcast(&self, addr: SocketAddr) -> ! {
        let mut interval = time::interval_at(
            time::Instant::now() + Duration::from_millis(HELLO_INTERVAL_MS),
            Duration::from_millis(HELLO_INTERVAL_MS),
        );

        loop {
            interval.tick().await;

            if let Err(e) = self
                .send_multicast_message(
                    Message::Hello(HelloPayload {
                        node_id: self.id,
                        unicast_port: self.connection.unicast_port,
                    }),
                    addr,
                )
                .await
                .context("failed to send broadcast message")
            {
                error!("error sending broadcast message: {}", e);
            }
        }
    }

    /// Periodic peer health monitoring loop.
    ///
    /// This method runs an infinite loop that sends ping messages to all established
    /// peers to monitor connection health and measure round-trip times. It only pings
    /// peers that have completed the handshake and are in transport mode.
    ///
    /// # Health Monitoring Process
    /// * **Peer Selection**: Only pings peers with established secure sessions
    /// * **Sequence Tracking**: Uses incrementing sequence numbers for RTT measurement
    /// * **Parallel Pings**: Sends pings to all peers concurrently
    /// * **Cleanup**: Removes stale ping records to prevent memory leaks
    ///
    /// # Ping Schedule
    /// * **Initial Delay**: Waits one full interval before first ping cycle
    /// * **Interval**: Pings every `PING_INTERVAL_MS` milliseconds (8 seconds)
    /// * **Persistence**: Continues pinging for the lifetime of the node
    ///
    /// # RTT Measurement
    /// Each ping includes:
    /// * Source node ID
    /// * Unique sequence number for matching with pong responses
    /// * Timestamp recorded in ping database for RTT calculation
    ///
    /// # Error Handling
    /// Individual ping failures are logged and counted, but don't stop the
    /// monitoring cycle. The method reports success/failure statistics.
    ///
    /// # Never Returns
    /// This method runs indefinitely until the process is terminated. It's designed
    /// to be spawned as a long-running async task.
    pub async fn ping_peers(&self) -> ! {
        let mut interval = time::interval_at(
            time::Instant::now() + Duration::from_millis(PING_INTERVAL_MS),
            Duration::from_millis(PING_INTERVAL_MS),
        );
        let link_db = self.state.link_db.clone();
        let ping_db = self.state.ping_db.clone();
        let session_db = self.state.session_db.clone();
        let mut sequence = 0u64;

        loop {
            interval.tick().await;
            sequence += 1;

            self.clean_stale_pings().await;

            let peers: Vec<_> = {
                let link_db = link_db.lock().await;
                let session_db = session_db.lock().await;

                link_db
                    .iter()
                    .filter_map(|(id, info)| {
                        session_db.get(id).and_then(|s| {
                            matches!(s.noise, NoiseState::Transport(_))
                                .then_some((*id, info.port, info.addr))
                        })
                    })
                    .collect()
            };

            if peers.is_empty() {
                info!("no peers to ping");
                continue;
            }

            let pings: Vec<_> = peers
                .iter()
                .map(|(id, port, addr)| {
                    self.send_ping(ping_db.clone(), *id, *port, sequence, *addr)
                })
                .collect();

            let results = join_all(pings).await;
            let success = results.iter().filter(|r| r.is_ok()).count();

            debug!(
                "successfully sent pings to {} peers, failed {} pings",
                success,
                results.len() - success
            );
        }
    }

    /// Handles incoming ping messages from peers.
    ///
    /// This method processes ping requests from other nodes and generates appropriate
    /// pong responses. It validates that the ping comes from a known peer with an
    /// established secure session before responding.
    ///
    /// # Arguments
    /// * `payload` - The ping message payload containing node ID and sequence number
    ///
    /// # Returns
    /// * `Ok(Some((port, pong_message)))` - Valid ping, returning pong response and target port
    /// * `Ok(None)` - Invalid ping (unknown peer or no secure session), no response
    /// * `Err(error)` - Processing error
    ///
    /// # Validation Process
    /// 1. Checks if the sender is a known peer in the link database
    /// 2. Verifies that a secure transport session exists with the peer
    /// 3. Retrieves the peer's unicast port for response routing
    ///
    /// # Response Generation
    /// The pong response includes:
    /// * This node's ID
    /// * The same sequence number from the ping (for RTT calculation)
    ///
    /// # Security
    /// Only responds to pings from peers with established secure sessions,
    /// preventing response to unauthorized or unauthenticated nodes.
    async fn handle_ping(&self, payload: &PingPongPayload) -> Result<Option<(u16, Message)>> {
        if !self.can_take_ping(payload).await {
            return Ok(None);
        }

        debug!(
            node_id = payload.node_id,
            sequence = payload.sequence,
            "received valid ping from peer",
        );

        let link_db = self.state.link_db.clone();
        let link_db = link_db.lock().await;
        let peer_info = match link_db.get(&payload.node_id) {
            Some(info) => info,
            None => {
                return Ok(None);
            }
        };
        let unicast_port = peer_info.port;

        let msg = PingPongPayload {
            node_id: self.id,
            sequence: payload.sequence,
        };

        Ok(Some((unicast_port, Message::Pong(msg))))
    }

    /// Handles incoming pong messages from peers.
    ///
    /// This method processes pong responses to previously sent ping messages,
    /// calculating round-trip times and updating peer statistics. It validates
    /// that the pong corresponds to a known outstanding ping request.
    ///
    /// # Arguments
    /// * `payload` - The pong message payload containing node ID and sequence number
    ///
    /// # Returns
    /// * `Ok(())` - Pong processed successfully or ignored (if invalid)
    /// * `Err(error)` - Processing error
    ///
    /// # RTT Calculation Process
    /// 1. Validates the pong against outstanding ping requests
    /// 2. Calculates RTT by comparing current time with ping timestamp
    /// 3. Updates the peer's RTT statistics with the new measurement
    /// 4. Removes the ping record from the tracking database
    ///
    /// # Statistics Update
    /// The RTT measurement is incorporated into the peer's statistics:
    /// * Current RTT value
    /// * Exponential moving average
    /// * Minimum and maximum RTT values
    /// * Sample count
    ///
    /// # Validation
    /// Only processes pongs that match outstanding ping requests with valid
    /// sequence numbers from known peers with secure sessions.
    async fn handle_pong(&self, payload: &PingPongPayload) -> Result<()> {
        let (k, ping_ts) = match self.can_take_pong(payload).await {
            Some((id, seq)) => (id, seq),
            None => return Ok(()),
        };

        debug!(
            node_id = k.0,
            sequence = k.1,
            "received valid pong from peer",
        );

        let link_db = self.state.link_db.clone();
        let mut link_db = link_db.lock().await;

        match link_db.get_mut(&k.0) {
            Some(p) => {
                p.rtt_stats
                    .get_or_insert_with(RttStats::default)
                    .update(Instant::now().duration_since(ping_ts));
                self.state.ping_db.lock().await.remove(&k);
            }
            None => {}
        }

        Ok(())
    }

    /// Validates whether a ping message should be processed.
    ///
    /// This method checks if an incoming ping message is from a known peer
    /// with an established secure session. It ensures that ping responses
    /// are only sent to authenticated and connected peers.
    ///
    /// # Arguments
    /// * `payload` - The ping message payload to validate
    ///
    /// # Returns
    /// * `true` - Ping is valid and should be processed
    /// * `false` - Ping should be ignored (unknown peer or no secure session)
    ///
    /// # Validation Criteria
    /// 1. **Known Peer**: The sender must be in the link state database
    /// 2. **Secure Session**: A transport-mode session must exist with the peer
    ///
    /// # Security Purpose
    /// This validation prevents the node from responding to pings from:
    /// * Unknown or unauthenticated nodes
    /// * Peers that haven't completed the handshake process
    /// * Peers with failed or incomplete secure sessions
    async fn can_take_ping(&self, payload: &PingPongPayload) -> bool {
        let link_db = self.state.link_db.clone();
        let session_db = self.state.session_db.clone();

        link_db.lock().await.contains_key(&payload.node_id)
            && matches!(
                session_db.lock().await.get(&payload.node_id),
                Some(PeerSession {
                    noise: NoiseState::Transport(_)
                }),
            )
    }

    /// Validates whether a pong message should be processed.
    ///
    /// This method checks if an incoming pong message corresponds to a valid
    /// outstanding ping request. It ensures that RTT calculations are only
    /// performed for legitimate ping/pong pairs.
    ///
    /// # Arguments
    /// * `payload` - The pong message payload to validate
    ///
    /// # Returns
    /// * `Some(((node_id, sequence), timestamp))` - Valid pong with ping info
    /// * `None` - Invalid pong that should be ignored
    ///
    /// # Validation Process
    /// 1. **Known Peer**: Sender must be in the link state database
    /// 2. **Secure Session**: Transport-mode session must exist with the peer
    /// 3. **Outstanding Ping**: A ping with matching sequence number must exist
    ///
    /// # Return Value
    /// When valid, returns:
    /// * `node_id` - The peer's node identifier
    /// * `sequence` - The ping sequence number
    /// * `timestamp` - When the original ping was sent (for RTT calculation)
    ///
    /// # Security and Integrity
    /// This validation prevents processing of:
    /// * Pongs from unknown or unauthenticated peers
    /// * Pongs without corresponding ping requests
    /// * Duplicate or replayed pong messages
    async fn can_take_pong(&self, payload: &PingPongPayload) -> Option<((NodeId, u64), Instant)> {
        let link_db = self.state.link_db.clone();
        let ping_db = self.state.ping_db.clone();
        let session_db = self.state.session_db.clone();

        if !link_db.lock().await.contains_key(&payload.node_id) {
            debug!(
                node_id = payload.node_id,
                "received ping/pong from unknown peer, ignoring"
            );
            return None;
        }

        if !matches!(
            session_db.lock().await.get(&payload.node_id),
            Some(PeerSession {
                noise: NoiseState::Transport(_)
            }),
        ) {
            info!(
                node_id = payload.node_id,
                "received ping/pong from peer without secure session, ignoring"
            );
            return None;
        }

        match ping_db
            .lock()
            .await
            .get(&(payload.node_id, payload.sequence))
        {
            Some(ping_ts) => {
                debug!(
                    node_id = payload.node_id,
                    sequence = payload.sequence,
                    "received ping/pong with known sequence",
                );
                Some(((payload.node_id, payload.sequence), *ping_ts))
            }
            None => {
                info!(
                    node_id = payload.node_id,
                    sequence = payload.sequence,
                    "received ping/pong with unknown sequence, ignoring",
                );
                None
            }
        }
    }

    /// Handles incoming Hello messages from new peers.
    ///
    /// This method processes Hello messages received from other nodes during
    /// peer discovery. It determines whether to initiate a handshake or wait
    /// for the peer to initiate based on node ID comparison.
    ///
    /// # Arguments
    /// * `payload` - The Hello message payload containing peer information
    /// * `src_addr` - The source network address of the Hello message
    /// * `session_db_recv` - Shared session database for state management
    ///
    /// # Returns
    /// * `Ok(Some(handshake_message))` - Initiating handshake, return handshake message
    /// * `Ok(None)` - Becoming responder, no message to send
    /// * `Err(MeshError)` - Processing error
    ///
    /// # Handshake Role Determination
    /// The node with the **higher** ID initiates the handshake:
    /// * **Higher ID**: Starts handshake by creating initiator state and sending first message
    /// * **Lower ID**: Becomes responder by creating expectant state and waiting
    ///
    /// # Session State Management
    /// * **Initiator**: Creates `Handshaking` state with Noise initiator
    /// * **Responder**: Creates `ExpectingInitiator` state to wait for handshake
    ///
    /// # Security
    /// Uses the node's static keys and network PSK to create secure handshake
    /// states that will establish authenticated, encrypted communication channels.
    async fn handle_hello(
        &self,
        payload: &HelloPayload,
        src_addr: SocketAddr,
        session_db_recv: SessionDb,
    ) -> Result<Option<Message>, MeshError> {
        info!(node_id = payload.node_id, "new peer @ {}", src_addr);

        if self.id < payload.node_id {
            info!(
                node_id = self.id,
                "our node id is less than peers, becoming responder"
            );

            let mut db = session_db_recv.lock().await;
            db.insert(
                payload.node_id,
                PeerSession {
                    noise: NoiseState::ExpectingInitiator,
                },
            );

            info!(
                node_id = payload.node_id,
                "session inserted with expecting initiator state for node",
            );

            return Ok(None);
        }

        info!(
            node_id = self.id,
            "our node id is greater than peers, starting handshake",
        );

        let (state, msg) = noise::start_handshake(&self.static_keys, &self.network_key)
            .context("failed to start handshake")?;

        let session_state: NoiseState = NoiseState::Handshaking(state);
        let mut db = session_db_recv.lock().await;
        db.insert(
            payload.node_id,
            PeerSession {
                noise: session_state,
            },
        );

        info!(
            node_id = payload.node_id,
            "session inserted with handshaking state for node"
        );

        let pl = HandshakePayload {
            node_id: self.id,
            msg,
            unicast_port: self.connection.unicast_port,
        };

        Ok(Some(Message::Handshake(pl)))
    }

    /// Handles incoming Noise protocol handshake messages.
    ///
    /// This method processes handshake messages during the Noise protocol
    /// session establishment. It manages the multi-step handshake process
    /// and transitions peers to secure transport mode upon completion.
    ///
    /// # Arguments
    /// * `HandshakePayload` - Contains node ID, handshake message bytes, and port
    /// * `session_db_recv` - Shared session database for state management
    /// * `src_addr` - Source network address of the handshake message
    ///
    /// # Returns
    /// * `Ok(Some(handshake_message))` - Continue handshake, return next message
    /// * `Ok(None)` - Handshake complete, no message to send
    /// * `Err(error)` - Handshake processing error
    ///
    /// # Handshake State Transitions
    /// * **ExpectingInitiator** → **Handshaking**: Process first message, send response
    /// * **Handshaking** → **Transport**: Complete handshake, establish secure channel
    /// * **Handshaking** → **Handshaking**: Continue multi-step handshake
    ///
    /// # Session Creation
    /// For unknown peers (if this node has lower ID):
    /// * Creates new responder session in `ExpectingInitiator` state
    /// * Processes the handshake message and transitions to `Handshaking`
    ///
    /// # Security Features
    /// * Mutual authentication using static keys
    /// * Forward secrecy through ephemeral key exchange
    /// * Network-level authentication via pre-shared key
    /// * Automatic transition to encrypted transport mode
    async fn handle_handshake(
        &self,
        HandshakePayload {
            node_id,
            msg,
            unicast_port: _unicast_port,
        }: HandshakePayload,
        session_db_recv: SessionDb,
        src_addr: SocketAddr,
    ) -> Result<Option<Message>> {
        info!(
            node_id = node_id,
            src_addr = %src_addr,
            msg_len = msg.len(),
            "received HANDSHAKE",
        );

        let mut db = session_db_recv.lock().await;
        let peer_session = match db.get_mut(&node_id) {
            Some(session) => session,
            None => {
                if self.id < node_id {
                    info!(
                        node_id = node_id,
                        "Creating responder session from unknown peer {}", node_id
                    );
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

        info!(node_id = node_id, "Found session for node {}", node_id);

        match std::mem::replace(&mut peer_session.noise, NoiseState::Transitioning) {
            NoiseState::ExpectingInitiator => {
                info!(
                    node_id = self.id,
                    "Node {}: acting as responder for handshake(EXPECTINGINITIATIOR)", self.id
                );
                let mut responder_state =
                    noise::build_responder(&self.static_keys, &self.network_key)?;

                let mut read_buf = [0u8; 1024];
                responder_state.read_message(&msg, &mut read_buf)?;

                let mut write_buf = [0u8; 1024];
                let len = responder_state.write_message(&[], &mut write_buf)?;

                peer_session.noise = NoiseState::Handshaking(responder_state);

                Ok(Some(Message::Handshake(HandshakePayload {
                    node_id: self.id,
                    msg: write_buf[..len].to_vec(),
                    unicast_port: self.connection.unicast_port,
                })))
            }
            NoiseState::Handshaking(mut state) => {
                info!(
                    node_id = self.id,
                    "continuing handshake with existing state(HANDSHAKING)",
                );

                let mut read_buf = [0u8; 1024];
                if let Err(e) = state.read_message(&msg, &mut read_buf) {
                    error!(error = %e, src_addr = %src_addr, "error reading message");
                    return Err(anyhow!("unable to read msg"));
                }

                if state.is_handshake_finished() {
                    info!(
                        node_id = node_id,
                        "handshake completed, session is now secure"
                    );

                    let transport_state = state
                        .into_transport_mode()
                        .context("error converting to transport mode")?;

                    peer_session.noise = NoiseState::Transport(transport_state);

                    return Ok(None);
                }

                let mut write_buf = [0u8; 1024];
                let len = match state.write_message(&[], &mut write_buf) {
                    Ok(len) => len,
                    Err(e) => {
                        error!(error = %e, "error writing message");
                        return Err(anyhow!("unable to write msg"));
                    }
                };

                if state.is_handshake_finished() {
                    info!(
                        node_id = node_id,
                        "handshake completed, session is now secure"
                    );

                    let transport_state = state
                        .into_transport_mode()
                        .context("error converting to transport mode")?;

                    peer_session.noise = NoiseState::Transport(transport_state);
                } else {
                    info!(
                        node_id = node_id,
                        "Handshake  not yet complete, still in handshaking state",
                    );
                    peer_session.noise = NoiseState::Handshaking(state);
                }

                Ok(Some(Message::Handshake(HandshakePayload {
                    node_id: self.id,
                    msg: write_buf[..len].to_vec(),
                    unicast_port: self.connection.unicast_port,
                })))
            }
            other_state => {
                let err = anyhow!("unexpected noise state: {:?}", other_state);
                peer_session.noise = other_state;
                Err(err)
            }
        }
    }

    /// Removes stale ping records from the ping database.
    ///
    /// This method performs cleanup of ping records that are older than the
    /// timeout threshold. It prevents the ping database from growing unbounded
    /// due to lost pong responses or network issues.
    ///
    /// # Cleanup Process
    /// * **Timeout**: Removes pings older than 30 seconds
    /// * **Memory Management**: Prevents database growth from lost pongs
    /// * **Automatic**: Called periodically during the ping cycle
    ///
    /// # Why Cleanup is Needed
    /// * Network packets can be lost
    /// * Peers may become unavailable
    /// * Handshake failures can leave stale records
    /// * Memory usage must be bounded
    ///
    /// # Timeout Rationale
    /// The 30-second timeout is chosen because:
    /// * Much longer than typical network RTTs
    /// * Allows for network congestion and delays
    /// * Prevents excessive memory usage
    /// * Balances cleanup frequency with tolerance for slow networks
    async fn clean_stale_pings(&self) {
        let ping_db = self.state.ping_db.clone();
        let mut ping_db = ping_db.lock().await;
        let timeout = Duration::from_secs(30);
        let now = Instant::now();

        ping_db.retain(|_, ping| now.duration_since(*ping) > timeout);
    }

    /// Periodic stale peer cleanup loop.
    ///
    /// This method runs an infinite loop that removes peer connections that haven't
    /// been heard from in a specified timeout period. It removes both the peer
    /// information and any associated session state to prevent resource leaks.
    ///
    /// # Cleanup Schedule
    /// * **Initial Delay**: Waits 30 seconds before first cleanup
    /// * **Interval**: Cleans up every 30 seconds
    /// * **Timeout**: Removes peers not seen for 60 seconds
    /// * **Persistence**: Continues cleaning for the lifetime of the node
    ///
    /// # Never Returns
    /// This method runs indefinitely until the process is terminated. It's designed
    /// to be spawned as a long-running async task.
    pub async fn monitor_peers(&self) -> ! {
        let mut interval = time::interval_at(
            time::Instant::now() + Duration::from_secs(30),
            Duration::from_secs(30),
        );

        loop {
            interval.tick().await;
            self.remove_stale_peers().await;
        }
    }

    /// Removes stale peer connections from the link and session databases.
    ///
    /// This method performs cleanup of peer connections that haven't been heard
    /// from in a specified timeout period. It removes both the peer information
    /// and any associated session state to prevent resource leaks.
    async fn remove_stale_peers(&self) {
        let link_db = self.state.link_db.clone();
        let session_db = self.state.session_db.clone();
        let timeout = Duration::from_secs(60);
        let now = Instant::now();

        let mut stale_peers = Vec::new();

        {
            let link_db = link_db.lock().await;
            for (peer_id, peer_info) in link_db.iter() {
                if now.duration_since(peer_info.last_seen) > timeout {
                    stale_peers.push(*peer_id);
                }
            }
        }

        if stale_peers.is_empty() {
            return;
        }

        info!("Removing {} stale peers", stale_peers.len());

        let mut link_db = link_db.lock().await;
        let mut session_db = session_db.lock().await;

        for peer_id in stale_peers {
            link_db.remove(&peer_id);
            session_db.remove(&peer_id);
        }
    }

    /// Sends a ping message to a specific peer.
    ///
    /// This method sends a ping message to a peer for connection health monitoring
    /// and RTT measurement. It records the ping timestamp for later RTT calculation
    /// when the corresponding pong is received.
    ///
    /// # Arguments
    /// * `ping_db` - Ping tracking database to record the ping timestamp
    /// * `node_id` - Target peer's node identifier
    /// * `unicast_port` - Target peer's unicast port
    /// * `sequence` - Unique sequence number for this ping
    /// * `addr` - Target peer's network address
    ///
    /// # Returns
    /// * `Ok(())` - Ping sent successfully
    /// * `Err(error)` - Network or serialization error
    ///
    /// # Process
    /// 1. Records ping timestamp in database with (node_id, sequence) key
    /// 2. Creates ping message with this node's ID and sequence number
    /// 3. Sends ping via unicast to the target peer
    ///
    /// # RTT Measurement
    /// The timestamp is used to calculate RTT when the pong response arrives:
    /// * Key: (target_node_id, sequence_number)
    /// * Value: Instant when ping was sent
    /// * Removed when pong is received or ping times out
    async fn send_ping(
        &self,
        ping_db: PingDb,
        node_id: NodeId,
        unicast_port: u16,
        sequence: u64,
        addr: SocketAddr,
    ) -> Result<()> {
        let mut ping_db = ping_db.lock().await;
        ping_db.insert((node_id, sequence), Instant::now());
        drop(ping_db);

        self.send_unicast_message(
            Message::Ping(PingPongPayload {
                node_id: self.id,
                sequence,
            }),
            unicast_port,
            addr,
        )
        .await
        .context(format!("failed to send ping message to node {}", node_id))
    }

    /// Sends a message to a specific peer via unicast.
    ///
    /// This method sends a message directly to a specific peer using their
    /// unicast address and port. It's used for peer-to-peer communication
    /// including handshakes, pings, pongs, and encrypted data.
    ///
    /// # Arguments
    /// * `msg` - The message to send
    /// * `port` - Target peer's unicast port
    /// * `addr` - Target peer's network address
    ///
    /// # Returns
    /// * `Ok(())` - Message sent successfully
    /// * `Err(error)` - Network or serialization error
    ///
    /// # Address Construction
    /// * Takes the source address from the received message
    /// * Replaces the port with the peer's unicast port
    /// * Ensures messages are sent to the correct endpoint
    ///
    /// # Message Types
    /// Used for sending:
    /// * Handshake messages during session establishment
    /// * Ping messages for health monitoring
    /// * Pong responses to ping requests
    /// * Encrypted data messages (future use)
    ///
    /// # Reliability
    /// Includes timeout handling and error logging for network resilience.
    async fn send_unicast_message(&self, msg: Message, port: u16, addr: SocketAddr) -> Result<()> {
        let mut unicast_addr = addr;
        unicast_addr.set_port(port);

        self.send_message(msg, unicast_addr, &self.connection.unicast_socket)
            .await
            .context(format!(
                "failed to send unicast message to {}",
                unicast_addr
            ))
    }

    /// Sends a message via multicast to all nodes in the network.
    ///
    /// This method broadcasts a message to the multicast group that all mesh
    /// nodes are listening on. It's primarily used for Hello messages during
    /// peer discovery.
    ///
    /// # Arguments
    /// * `msg` - The message to broadcast
    /// * `addr` - The multicast address to send to
    ///
    /// # Returns
    /// * `Ok(())` - Message broadcast successfully
    /// * `Err(error)` - Network or serialization error
    ///
    /// # Usage
    /// Currently used for:
    /// * Hello message broadcasting for peer discovery
    /// * Network-wide announcements (future use)
    ///
    /// # Multicast Behavior
    /// * Sends to all nodes in the multicast group
    /// * Nodes filter out their own messages by ID
    /// * Provides efficient one-to-many communication
    /// * Limited to local network segment by default
    ///
    /// # Network Considerations
    /// * Multicast may not traverse all network boundaries
    /// * Reliability depends on network configuration
    /// * UDP-based, so delivery is not guaranteed
    async fn send_multicast_message(&self, msg: Message, addr: SocketAddr) -> Result<()> {
        self.send_message(msg, addr, &self.connection.multicast_socket)
            .await
            .context(format!("failed to send multicast message to {}", addr))
    }

    async fn send_message(
        &self,
        msg: Message,
        addr: SocketAddr,
        socket: &Arc<UdpSocket>,
    ) -> Result<()> {
        if let Err(e) = time::timeout(
            Duration::from_secs(5),
            socket.send_to(
                &bincode::serialize(&msg).context("error serializing message")?,
                addr,
            ),
        )
        .await
        {
            error!("timeout sending message to {}: {}", addr, e);
            return Err(anyhow!("failed to send message to {}: {}", addr, e));
        }

        Ok(())
    }

    /// Periodic metrics collection loop.
    ///
    /// This method runs an infinite loop that collects and updates metrics
    /// at regular intervals. It should be spawned as a separate async task.
    pub async fn collect_metrics(&self, metrics: Arc<Metrics>) -> ! {
        let dur = Duration::from_secs(5);
        let mut interval = time::interval_at(time::Instant::now() + dur, dur);
        info!("starting metrics collection task");

        loop {
            interval.tick().await;

            if let Err(e) = self.update_metrics(&metrics).await {
                error!("Error collecting metrics: {}", e);
            }
        }
    }

    /// Collects and updates metrics from the current node state.
    ///
    /// This method gathers RTT statistics and connection information
    /// from the node's state databases and updates the Prometheus metrics.
    /// It should be called periodically to keep metrics current.
    pub async fn update_metrics(&self, metrics: &Metrics) -> Result<()> {
        let link_db = self.state.link_db.lock().await;
        let session_db = self.state.session_db.lock().await;

        let local_ip = self
            .connection
            .unicast_socket
            .local_addr()?
            .ip()
            .to_string();
        let mut connected_count = 0;

        // only care about established peers with RTT stats
        for (peer_id, peer_info) in link_db.iter() {
            let session = session_db.get(peer_id);
            if session.is_none()
                || !matches!(session.unwrap().noise, mesh::NoiseState::Transport(_))
                || peer_info.rtt_stats.is_none()
            {
                continue;
            }

            connected_count += 1;

            let rtt_stats = peer_info.rtt_stats.as_ref().unwrap();
            let remote_ip = peer_info.addr.ip().to_string();
            metrics.update_peer_rtt(self.id, *peer_id, &local_ip, &remote_ip, rtt_stats);
        }

        metrics.update_connected_peers_count(
            connected_count,
            local_ip.as_str(),
            self.id,
            self.metrics_port,
        );

        Ok(())
    }

    /// Returns the node's unique identifier.
    pub fn get_id(&self) -> u32 {
        self.id
    }

    /// Returns the node's local IP address.
    pub fn get_local_ip(&self) -> String {
        format!("{}:{}", self.ip.clone(), self.connection.unicast_port)
    }
}
