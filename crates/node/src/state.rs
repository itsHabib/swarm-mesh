use mesh::{NodeId, PeerInfo, PeerSession};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Type alias for the link state database.
///
/// This database maintains information about all discovered peers in the mesh network.
/// It maps node IDs to their connection information, including network addresses,
/// ports, and RTT statistics. The database is thread-safe and can be accessed
/// concurrently by multiple async tasks.
///
/// # Key: NodeId
/// The unique identifier for each peer node.
///
/// # Value: PeerInfo
/// Complete connection and performance information for the peer.
///
/// # Thread Safety
/// Wrapped in Arc<Mutex<>> to allow safe concurrent access across async tasks.
pub type LinkStateDb = Arc<Mutex<HashMap<NodeId, PeerInfo>>>;

/// Type alias for the session database.
///
/// This database tracks the cryptographic session state for each peer connection.
/// It maintains the Noise protocol handshake and transport states, allowing the
/// node to manage secure communications with multiple peers simultaneously.
///
/// # Key: NodeId
/// The unique identifier for each peer node.
///
/// # Value: PeerSession
/// The cryptographic session state and Noise protocol information.
///
/// # Thread Safety
/// Wrapped in Arc<Mutex<>> to allow safe concurrent access across async tasks.
pub type SessionDb = Arc<Mutex<HashMap<NodeId, PeerSession>>>;

/// Type alias for the ping tracking database.
///
/// This database tracks outstanding ping requests to measure round-trip times
/// and monitor connection health. It stores the timestamp when each ping was
/// sent, keyed by both the target node ID and the ping sequence number.
///
/// # Key: (NodeId, u64)
/// A tuple of the target node ID and the ping sequence number.
///
/// # Value: std::time::Instant
/// The timestamp when the ping was sent.
///
/// # Thread Safety
/// Wrapped in Arc<Mutex<>> to allow safe concurrent access across async tasks.
///
/// # Cleanup
/// Entries are automatically removed when pong responses are received or
/// when pings timeout to prevent unbounded memory growth.
pub type PingDb = Arc<Mutex<HashMap<(NodeId, u64), std::time::Instant>>>;

/// Central state management for a mesh node.
///
/// This struct consolidates all the stateful information that a mesh node
/// needs to maintain during its operation. It provides a single point of
/// access to peer information, session states, and ping tracking.
///
/// # State Components
/// * **Link Database**: Peer discovery and connection information
/// * **Session Database**: Cryptographic session states
/// * **Ping Database**: RTT measurement and connection health tracking
///
/// # Design Philosophy
/// The state is designed to be shared across multiple async tasks that
/// handle different aspects of mesh networking (discovery, handshakes,
/// ping/pong, data transmission). Each database is independently lockable
/// to minimize contention.
pub struct State {
    /// Database of discovered peers and their connection information
    pub(crate) link_db: LinkStateDb,
    /// Database of cryptographic sessions with peers
    pub(crate) session_db: SessionDb,
    /// Database of outstanding ping requests for RTT measurement
    pub(crate) ping_db: PingDb,
}

impl State {
    /// Creates a new State instance with the provided databases.
    ///
    /// This constructor takes pre-initialized databases and combines them
    /// into a single state management structure. This design allows for
    /// flexible initialization and testing scenarios.
    ///
    /// # Arguments
    /// * `link_db` - Thread-safe database for peer connection information
    /// * `session_db` - Thread-safe database for cryptographic sessions
    /// * `ping_db` - Thread-safe database for ping tracking
    ///
    /// # Returns
    /// A new State instance ready for mesh node operations
    ///
    /// # Usage
    /// Typically called during node initialization after creating empty
    /// HashMaps wrapped in Arc<Mutex<>> for thread-safe access.
    pub fn new(link_db: LinkStateDb, session_db: SessionDb, ping_db: PingDb) -> Self {
        Self {
            link_db,
            session_db,
            ping_db,
        }
    }
}
