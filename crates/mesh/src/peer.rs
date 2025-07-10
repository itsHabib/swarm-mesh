use crate::rtt::RttStats;
use snow::{HandshakeState, TransportState};
use std::net::SocketAddr;
use std::time::Instant;

pub type NodeId = u32;

pub struct PeerInfo {
    pub last_seen: Instant,
    pub addr: SocketAddr,
    pub port: u16,
    pub rtt_stats: Option<RttStats>,
}

impl PeerInfo {
    pub fn new(addr: SocketAddr, port: u16) -> Self {
        Self {
            last_seen: Instant::now(),
            addr,
            port,
            rtt_stats: None,
        }
    }

    pub fn update(&mut self, addr: SocketAddr, port: u16) {
        self.last_seen = Instant::now();
        self.addr = addr;
        self.port = port;
    }
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
