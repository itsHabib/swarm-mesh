use snow::{HandshakeState, TransportState};
use std::net::SocketAddr;
use std::time::Instant;

pub type NodeId = u32;

pub struct PeerInfo {
    pub last_seen: Instant,
    pub addr: SocketAddr,
    pub port: u16,
}

pub enum PeerState {
    HandshakeInProgress { state: HandshakeState },
    Established { cipher: TransportState },
}

pub struct PeerSession {
    pub noise: crate::NoiseState,
}

#[derive(Debug)]
pub enum NoiseState {
    ExpectingInitiator,
    Handshaking(HandshakeState),
    Transport(TransportState),
    Transitioning,
}
