use anyhow::{Context, Error, Result};
use mesh::NodeId;
use serde::Deserialize;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::ops::{Sub, SubAssign};
use std::sync::{Arc, Mutex, MutexGuard};

/// A time-based registry for tracking active mesh nodes.
///
/// The Registry maintains a collection of nodes that have registered themselves
/// along with their last-seen timestamps. It provides automatic cleanup of stale
/// nodes based on a configurable TTL (Time To Live) duration.
///
/// # Design
/// * Uses a binary heap for efficient TTL-based cleanup
/// * Stores multiple entries per node to track registration history
/// * Automatically purges expired entries to prevent memory leaks
/// * Returns only the most recent entry for each node ID
///
/// # Thread Safety
/// The Registry is not thread-safe by itself. It should be wrapped in
/// appropriate synchronization primitives (like Arc<Mutex<>>) when used
/// in concurrent environments.
pub struct Registry {
    /// Time-to-live duration for node entries
    ttl: std::time::Duration,
    /// Binary heap of node entries, ordered by timestamp for efficient cleanup
    nodes: BinaryHeap<NodeEntry>,
}

/// Information about a mesh node for registry purposes.
///
/// This struct represents the essential information needed to track and
/// communicate with a mesh node. It includes the node's identity and
/// network location details.
#[derive(PartialEq, Eq, Clone, Deserialize)]
pub struct Node {
    /// Unique identifier for the mesh node
    pub id: NodeId,
    /// IP address where the node can be reached
    pub ip: String,
    /// Port number where the node exposes Prometheus metrics
    pub metrics_port: u16,
}

/// A timestamped entry for a node in the registry.
///
/// This struct wraps a Node with a timestamp indicating when it was last seen.
/// It's used internally by the Registry to track node activity and implement
/// TTL-based cleanup. The ordering is based on the timestamp for efficient
/// heap operations.
#[derive(PartialEq, Eq, Clone)]
pub struct NodeEntry {
    /// The node information
    pub node: Node,
    /// When this entry was created/last updated
    pub seen: std::time::Instant,
}

impl Ord for NodeEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other.seen.cmp(&self.seen)
    }
}

impl PartialOrd for NodeEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Registry {
    /// Creates a new Registry with the specified TTL duration.
    ///
    /// # Example
    /// ```
    /// use std::time::Duration;
    /// use registry::Registry;
    /// 
    /// let registry = Registry::new(Duration::from_secs(300)); // 5 minute TTL
    /// ```
    pub fn new(ttl: std::time::Duration) -> Self {
        Registry {
            ttl,
            nodes: BinaryHeap::new(),
        }
    }

    /// Adds or updates a node in the registry.
    ///
    /// This method registers a node with the current timestamp. If the node
    /// already exists, this creates a new entry with the updated timestamp,
    /// effectively refreshing the node's TTL.
    ///
    /// # Example
    /// ```
    /// use registry::{Registry, Node};
    /// use std::time::Duration;
    /// 
    /// let mut registry = Registry::new(Duration::from_secs(300));
    /// let node = Node {
    ///     id: 12345,
    ///     ip: "192.168.1.100".to_string(),
    ///     metrics_port: 8080,
    /// };
    /// registry.add(node);
    /// ```
    pub fn add(&mut self, node: Node) {
        self.nodes.push(NodeEntry {
            node,
            seen: std::time::Instant::now(),
        });
    }

    /// Returns the most recent entry for each registered node.
    ///
    /// This method processes all entries in the registry and returns only the
    /// most recent entry for each unique node ID. This is useful for getting
    /// the current state of all registered nodes without duplicates.
    ///
    /// # Example
    /// ```
    /// use registry::{Registry, Node};
    /// use std::time::Duration;
    /// 
    /// let mut registry = Registry::new(Duration::from_secs(300));
    /// // ... add some nodes ...
    /// let active_nodes = registry.nodes();
    /// println!("Found {} active nodes", active_nodes.len());
    /// ```
    pub fn nodes(&self) -> Vec<NodeEntry> {
        let mut latest: HashMap<NodeId, NodeEntry> = HashMap::new();

        for cur in self.nodes.iter() {
            latest
                .entry(cur.node.id)
                .and_modify(|e| {
                    if cur.seen.gt(&e.seen) {
                        *e = cur.clone();
                    }
                })
                .or_insert(cur.clone());
        }

        latest.into_values().collect()
    }

    /// Removes expired entries from the registry.
    ///
    /// This method cleans up entries that have exceeded their TTL (Time To Live)
    /// duration. It removes entries from the front of the heap (oldest entries)
    /// until it encounters an entry that is still valid.
    ///
    /// # Usage
    /// This method should be called periodically to prevent unbounded memory
    /// growth. The mesh-registry service calls this automatically in a
    /// background task.
    ///
    /// # Example
    /// ```
    /// use registry::{Registry, Node};
    /// use std::time::Duration;
    /// 
    /// let mut registry = Registry::new(Duration::from_secs(300));
    /// // ... add some nodes and wait ...
    /// registry.purge(); // Remove expired entries
    /// ```
    pub fn purge(&mut self) {
        let expired =
            |e: &NodeEntry| -> bool { e.seen.le(&std::time::Instant::now().sub(self.ttl)) };

        while let Some(entry) = self.nodes.peek()
            && expired(&entry)
        {
            self.nodes.pop();
        }
    }
}
