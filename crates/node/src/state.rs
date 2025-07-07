use mesh::{NodeId, PeerInfo, PeerSession};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

pub type LinkStateDb = Arc<Mutex<HashMap<NodeId, PeerInfo>>>;
pub type SessionDb = Arc<Mutex<HashMap<NodeId, PeerSession>>>;

pub struct State {
    pub(crate) link_db: LinkStateDb,
    pub(crate) session_db: SessionDb,
}

impl State {
    pub fn new(link_db: LinkStateDb, session_db: SessionDb) -> Self {
        Self {
            link_db,
            session_db,
        }
    }
}
