use crate::{
    connection::Connection,
    state::{PingDb, SessionDb, State},
};
use anyhow::{Context, Result, anyhow};
use futures::future::join_all;
use mesh::{
    Error as MeshError, HandshakePayload, HelloPayload, Message, NodeId, NoiseState, PeerInfo,
    PeerSession,
};
use noise;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::time;
use tracing::{error, info};

const HELLO_INTERVAL_MS: u64 = 15000;
const PING_INTERVAL_MS: u64 = 8000;

pub struct Node {
    id: u32,
    network_key: [u8; 32],
    static_keys: snow::Keypair,
    connection: Connection,
    state: State,
}

impl Node {
    pub fn new(
        id: u32,
        network_key: [u8; 32],
        static_keys: snow::Keypair,
        connection: Connection,
        state: State,
    ) -> Self {
        Node {
            id,
            network_key,
            static_keys,
            connection,
            state,
        }
    }

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
                                p.last_seen = Instant::now();
                                p.addr = src_addr;
                                p.port = payload.unicast_port;
                            })
                            .or_insert(PeerInfo {
                                last_seen: Instant::now(),
                                addr: src_addr,
                                port: payload.unicast_port,
                            });
                        continue;
                    }

                    match self
                        .handle_hello(&payload, src_addr, session_db_recv.clone())
                        .await
                    {
                        Ok(Some(msg)) => {
                            let mut unicast_addr = src_addr;
                            unicast_addr.set_port(payload.unicast_port);
                            println!("Sending start handshake response to {}", unicast_addr);

                            if let Err(e) = self
                                .send_unicast_message(msg, payload.unicast_port, src_addr)
                                .await
                            {
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
                        Ok(msg) => msg,
                        Err(e) => {
                            eprintln!("Error handling handshake: {}", e);
                            continue; // Skip sending a message
                        }
                    };

                    let mut unicast_addr = src_addr;
                    unicast_addr.set_port(unicast_port);
                    println!("Sending handshake response to {}", unicast_addr);

                    if let Err(e) = self
                        .send_unicast_message(msg, unicast_port, unicast_addr)
                        .await
                    {
                        eprintln!("Error sending handshake response: {}", e);
                    }
                }
                _ => {
                    println!("Received unknown message from {}", src_addr);
                }
            }
        }
    }

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

    pub async fn ping_peers(&self) -> ! {
        let mut interval = time::interval_at(
            time::Instant::now() + Duration::from_millis(PING_INTERVAL_MS),
            Duration::from_millis(PING_INTERVAL_MS),
        );
        let link_db = self.state.link_db.clone();
        let ping_db = self.state.ping_db.clone();
        let mut sequence = 0u64;

        loop {
            interval.tick().await;
            sequence += 1;

            self.clean_stale_pings().await;

            let peers: Vec<_> = {
                let db = link_db.lock().await;
                db.iter()
                    .map(|(id, info)| (*id, info.port, info.addr))
                    .collect()
            };

            if peers.is_empty() {
                info!("No peers to ping");
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
            info!(
                "successfully sent pings to {} peers, failed {} pings",
                success,
                results.len() - success
            );
        }
    }

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

    async fn handle_handshake(
        &self,
        HandshakePayload {
            node_id,
            msg,
            unicast_port: udp_port,
        }: HandshakePayload,
        session_db_recv: SessionDb,
        src_addr: SocketAddr,
    ) -> Result<Message> {
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
                if self.id < node_id {
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
                    self.id
                );
                let mut responder_state =
                    noise::build_responder(&self.static_keys, &self.network_key)?;

                let mut read_buf = [0u8; 1024];
                responder_state.read_message(&msg, &mut read_buf)?;

                let mut write_buf = [0u8; 1024];
                let len = responder_state.write_message(&[], &mut write_buf)?;

                peer_session.noise = NoiseState::Handshaking(responder_state);

                Ok(Message::Handshake(HandshakePayload {
                    node_id: self.id,
                    msg: write_buf[..len].to_vec(),
                    unicast_port: udp_port,
                }))
            }
            NoiseState::Handshaking(mut state) => {
                println!(
                    "Node {}: continuing handshake with existing state(HANDSHAKING)",
                    self.id
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

                Ok(Message::Handshake(HandshakePayload {
                    node_id: self.id,
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

    async fn clean_stale_pings(&self) {
        let ping_db = self.state.ping_db.clone();
        let mut ping_db = ping_db.lock().await;
        let timeout = Duration::from_secs(30);
        let now = Instant::now();

        ping_db.retain(|_, ping| now.duration_since(*ping) > timeout);
    }

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
            Message::Ping(mesh::PingPayload { node_id, sequence }),
            unicast_port,
            addr,
        )
        .await
        .context(format!("failed to send ping message to node {}", node_id))
    }

    async fn send_unicast_message(&self, msg: Message, port: u16, addr: SocketAddr) -> Result<()> {
        let mut unicast_addr = addr;
        unicast_addr.set_port(port);

        self.send_message(msg, addr, &self.connection.unicast_socket)
            .await
            .context(format!("failed to send unicast message to {}", addr))
    }

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
}
