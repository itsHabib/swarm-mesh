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
                Message::Ping(payload) => {
                    if payload.node_id == self.id {
                        continue;
                    }
                    info!(
                        node_id = payload.node_id,
                        sequence = payload.sequence,
                        src_addr = %src_addr,
                        "received ping!!! from peer",
                    );

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

            info!(
                "successfully sent pings to {} peers, failed {} pings",
                success,
                results.len() - success
            );
        }
    }

    async fn handle_ping(&self, payload: &PingPongPayload) -> Result<Option<(u16, Message)>> {
        if !self.can_take_ping(payload).await {
            return Ok(None);
        }

        info!(
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

    async fn handle_pong(&self, payload: &PingPongPayload) -> Result<()> {
        let (k, ping_ts) = match self.can_take_pong(payload).await {
            Some((id, seq)) => (id, seq),
            None => return Ok(()),
        };

        info!(
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

    async fn can_take_pong(&self, payload: &PingPongPayload) -> Option<((NodeId, u64), Instant)> {
        let link_db = self.state.link_db.clone();
        let ping_db = self.state.ping_db.clone();
        let session_db = self.state.session_db.clone();

        if !link_db.lock().await.contains_key(&payload.node_id) {
            info!(
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
                info!(
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

    async fn send_unicast_message(&self, msg: Message, port: u16, addr: SocketAddr) -> Result<()> {
        let mut unicast_addr = addr;
        unicast_addr.set_port(port);

        info!(
            dest_addr = %unicast_addr,
            "attempting to send unicast message"
        );

        self.send_message(msg, unicast_addr, &self.connection.unicast_socket)
            .await
            .context(format!(
                "failed to send unicast message to {}",
                unicast_addr
            ))
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
