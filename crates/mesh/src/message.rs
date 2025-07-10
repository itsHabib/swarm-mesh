use crate::NodeId;
use serde::{Deserialize, Serialize};
use snow::HandshakeState;

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
    Ping(PingPongPayload),
    Pong(PingPongPayload),
    EncryptedData(Vec<u8>),
}

#[derive(Serialize, Deserialize, Clone)]
pub struct PingPongPayload {
    pub node_id: NodeId,
    pub sequence: u64,
}
