use anyhow::{Context, Error, Result};
use mesh::NodeId;
use serde::Deserialize;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::ops::{Sub, SubAssign};
use std::sync::{Arc, Mutex, MutexGuard};

pub struct Registry {
    ttl: std::time::Duration,
    nodes: BinaryHeap<NodeEntry>,
}

#[derive(PartialEq, Eq, Clone, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub ip: String,
    pub metrics_port: u16,
}

#[derive(PartialEq, Eq, Clone)]
pub struct NodeEntry {
    pub node: Node,
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
    pub fn new(ttl: std::time::Duration) -> Self {
        Registry {
            ttl,
            nodes: BinaryHeap::new(),
        }
    }

    pub fn add(&mut self, node: Node) {
        self.nodes.push(NodeEntry {
            node,
            seen: std::time::Instant::now(),
        });
    }

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
