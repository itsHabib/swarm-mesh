use std::sync::Arc;
use tokio::net::UdpSocket;

/// Network connection management for a mesh node.
/// 
/// This struct encapsulates the UDP sockets and port information needed
/// for a node to participate in the mesh network. It manages both multicast
/// communication (for peer discovery) and unicast communication (for direct
/// peer-to-peer messaging).
/// 
/// # Socket Management
/// * **Multicast Socket**: Used for broadcasting Hello messages and receiving
///   discovery messages from other nodes
/// * **Unicast Socket**: Used for direct communication with specific peers,
///   including handshakes, pings, and encrypted data
/// 
/// # Thread Safety
/// All sockets are wrapped in `Arc` to allow safe sharing across async tasks
/// and threads in the tokio runtime.
pub struct Connection {
    /// The UDP port number where this node listens for unicast messages
    pub(crate) unicast_port: u16,
    /// Shared UDP socket for unicast communication with peers
    pub(crate) unicast_socket: Arc<UdpSocket>,
    /// Shared UDP socket for multicast discovery communication
    pub(crate) multicast_socket: Arc<UdpSocket>,
}

impl Connection {
    /// Creates a new Connection instance with the provided sockets and port.
    /// 
    /// This constructor takes ownership of the socket configuration and
    /// wraps them for use in the mesh node's networking operations.
    /// 
    /// # Arguments
    /// * `unicast_port` - The port number for unicast communication
    /// * `unicast_socket` - Arc-wrapped UDP socket for unicast messages
    /// * `multicast_socket` - Arc-wrapped UDP socket for multicast discovery
    /// 
    /// # Returns
    /// A new Connection instance ready for mesh networking operations
    /// 
    /// # Usage
    /// This constructor is typically called during node initialization
    /// after the sockets have been properly configured with multicast
    /// groups and socket options.
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
