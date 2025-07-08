use crate::NodeId;
use serde::{Deserialize, Serialize};

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
    Ping(PingPayload),
    Pong(PongPayload),
    EncryptedData(Vec<u8>),
}

#[derive(Serialize, Deserialize, Clone)]
pub struct PingPayload {
    pub node_id: NodeId,
    pub sequence: u64,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct PongPayload {
    pub node_id: NodeId,
    pub sequence: u64,
    pub ping_timestamp: u64,
    pub pong_timestamp: u64,
}
