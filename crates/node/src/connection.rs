use std::sync::Arc;
use tokio::net::UdpSocket;

pub struct Connection {
    pub(crate) unicast_port: u16,
    pub(crate) unicast_socket: Arc<UdpSocket>,
    pub(crate) multicast_socket: Arc<UdpSocket>,
}

impl Connection {
    pub fn new(
        unicast_port: u16,
        unicast_socket: Arc<UdpSocket>,
        multicast_socket: Arc<UdpSocket>,
    ) -> Self {
        Self {
            unicast_port,
            unicast_socket,
            multicast_socket,
        }
    }
}
