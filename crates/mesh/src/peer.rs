use crate::rtt::RttStats;
use snow::{HandshakeState, TransportState};
use std::net::SocketAddr;
use std::time::Instant;

/// Type alias for node identifiers.
///
/// Each node in the mesh network has a unique 32-bit identifier that is used
/// for routing, session management, and determining handshake roles.
pub type NodeId = u32;

/// Information about a discovered peer node.
///
/// This struct maintains the essential connection information and metadata
/// for each peer that has been discovered in the mesh network. It tracks
/// when the peer was last seen, how to reach it, and network performance metrics.
pub struct PeerInfo {
    /// Timestamp when this peer was last seen (via Hello message or other communication)
    pub last_seen: Instant,
    /// Network address where this peer was last observed
    pub addr: SocketAddr,
    /// UDP port number where this peer listens for unicast messages
    pub port: u16,
    /// Round-trip time statistics for this peer (if available)
    pub rtt_stats: Option<RttStats>,
}

impl PeerInfo {
    /// Creates a new PeerInfo instance for a discovered peer.
    ///
    /// # Arguments
    /// * `addr` - The network address where the peer was discovered
    /// * `port` - The UDP port where the peer listens for unicast messages
    ///
    /// # Returns
    /// A new PeerInfo instance with current timestamp and no RTT stats
    pub fn new(addr: SocketAddr, port: u16) -> Self {
        Self {
            last_seen: Instant::now(),
            addr,
            port,
            rtt_stats: None,
        }
    }

    /// Updates the peer information with new address and port.
    ///
    /// This method is called when a peer is rediscovered, updating the
    /// last seen timestamp and potentially new network location.
    ///
    /// # Arguments
    /// * `addr` - The new network address for the peer
    /// * `port` - The new UDP port for the peer
    pub fn update(&mut self, addr: SocketAddr, port: u16) {
        self.last_seen = Instant::now();
        self.addr = addr;
        self.port = port;
    }
}

/// Represents the state of a peer's connection lifecycle.
///
/// This enum tracks whether a peer connection is in the process of
/// establishing a secure session or has already completed the handshake.
///
/// # Variants
/// * `HandshakeInProgress` - Currently performing Noise protocol handshake
/// * `Established` - Handshake complete, ready for encrypted communication
pub enum PeerState {
    /// Handshake is in progress with the given Noise handshake state
    HandshakeInProgress {
        /// The current Noise protocol handshake state
        state: HandshakeState,
    },
    /// Secure session established with transport cipher ready for encryption/decryption
    Established {
        /// The Noise protocol transport state for encrypted communication
        cipher: TransportState,
    },
}

/// Session information for a peer connection.
///
/// This struct encapsulates the cryptographic session state for a peer,
/// tracking the current phase of the Noise protocol handshake and session.
pub struct PeerSession {
    /// The current state of the Noise protocol session
    pub noise: NoiseState,
}

/// Represents the various states of a Noise protocol session.
///
/// The Noise protocol handshake progresses through several states before
/// reaching a fully established encrypted transport session. This enum
/// tracks the current phase of that progression.
#[derive(Debug)]
pub enum NoiseState {
    /// Waiting for the peer to initiate the handshake (responder role)
    ExpectingInitiator,
    /// Currently performing the multi-step handshake process
    Handshaking(HandshakeState),
    /// Handshake complete, ready for encrypted data transport
    Transport(TransportState),
    /// Temporary state used during state transitions
    Transitioning,
}
