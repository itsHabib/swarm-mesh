use serde::{Deserialize, Serialize};
use snow::{HandshakeState, TransportState};
use std::net::SocketAddr;
use std::time::Instant;
use thiserror::Error;

pub type NodeId = u32;

#[derive(Serialize, Deserialize, Clone)]
pub struct HelloPayload {
    pub node_id: NodeId,
    pub unicast_port: u16,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct HandshakePayload {
    pub msg: Vec<u8>,
    pub node_id: NodeId,
    pub unicast_port: u16,
}

#[derive(Serialize, Deserialize, Clone)]
pub enum Message {
    Hello(HelloPayload),
    Handshake(HandshakePayload),
    EncryptedData(Vec<u8>),
}

pub struct PeerInfo {
    pub last_seen: Instant,
    pub addr: SocketAddr,
}

pub enum PeerState {
    HandshakeInProgress { state: HandshakeState },
    Established { cipher: TransportState },
}

pub struct PeerSession {
    pub noise: NoiseState,
}

#[derive(Debug)]
pub enum NoiseState {
    ExpectingInitiator,
    Handshaking(HandshakeState),
    Transport(TransportState),
    Transitioning,
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("already connected to peer")]
    AlreadyConnected,
    #[error("becoming responder")]
    BecomingResponder,
    #[error("error {0}")]
    Other(#[from] anyhow::Error),
}
