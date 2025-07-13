use crate::NodeId;
use serde::{Deserialize, Serialize};

/// Payload for Hello messages used in peer discovery.
///
/// Hello messages are broadcast on multicast to announce a node's presence
/// and allow other nodes to discover it. Contains essential information
/// needed for establishing unicast communication.
#[derive(Serialize, Deserialize, Clone)]
pub struct HelloPayload {
    /// Unique identifier for the node sending the Hello message
    pub node_id: NodeId,
    /// UDP port number where the node listens for unicast messages
    pub unicast_port: u16,
}

/// Payload for Noise protocol handshake messages.
///
/// Contains the cryptographic handshake data needed to establish
/// a secure, authenticated connection between two nodes using the
/// Noise protocol framework.
#[derive(Serialize, Deserialize, Clone)]
pub struct HandshakePayload {
    /// The Noise protocol handshake message bytes
    pub msg: Vec<u8>,
    /// Unique identifier for the node sending the handshake
    pub node_id: NodeId,
    /// UDP port number where the node listens for unicast messages
    pub unicast_port: u16,
}

/// All possible message types in the mesh network protocol.
///
/// This enum defines the complete set of messages that can be exchanged
/// between nodes in the mesh network. Each message type serves a specific
/// purpose in the network protocol.
#[derive(Serialize, Deserialize, Clone)]
pub enum Message {
    /// Discovery message broadcast on multicast for peer discovery
    Hello(HelloPayload),
    /// Noise protocol handshake message for establishing secure sessions
    Handshake(HandshakePayload),
    /// Ping message for connection health monitoring and RTT measurement
    Ping(PingPongPayload),
    /// Pong response message for ping requests
    Pong(PingPongPayload),
    /// Encrypted application data sent over established secure channels
    EncryptedData(Vec<u8>),
}

/// Payload for ping and pong messages used in connection monitoring.
///
/// Ping/pong messages serve dual purposes: they verify that connections
/// are still alive and measure round-trip time (RTT) for network
/// performance monitoring.
#[derive(Serialize, Deserialize, Clone)]
pub struct PingPongPayload {
    /// Unique identifier for the node sending the ping/pong
    pub node_id: NodeId,
    /// Sequence number to match ping requests with pong responses
    pub sequence: u64,
}
