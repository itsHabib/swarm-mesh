use mesh::{NodeId, PeerInfo, PeerSession};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

pub type LinkStateDb = Arc<Mutex<HashMap<NodeId, PeerInfo>>>;
pub type SessionDb = Arc<Mutex<HashMap<NodeId, PeerSession>>>;
pub type PingDb = Arc<Mutex<HashMap<(NodeId, u64), std::time::Instant>>>;

pub struct State {
    pub(crate) link_db: LinkStateDb,
    pub(crate) session_db: SessionDb,
    pub(crate) ping_db: PingDb,
}

impl State {
    pub fn new(link_db: LinkStateDb, session_db: SessionDb, ping_db: PingDb) -> Self {
        Self {
            link_db,
            session_db,
            ping_db,
        }
    }
}
