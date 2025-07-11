use thiserror::Error;

/// Error types that can occur during mesh network operations.
///
/// This enum defines the specific error conditions that can arise
/// during mesh network communication and connection management.
/// It uses the `thiserror` crate for ergonomic error handling.
#[derive(Debug, Error)]
pub enum Error {
    /// Attempted to connect to a peer that already has an established connection.
    ///
    /// This error occurs when the system tries to initiate a new connection
    /// to a peer that is already connected, preventing duplicate connections.
    #[error("already connected to peer")]
    AlreadyConnected,

    /// The node is transitioning to a responder role in the handshake process.
    ///
    /// This error indicates that the node has determined it should act as
    /// the responder (rather than initiator) in the Noise protocol handshake
    /// based on node ID comparison.
    #[error("becoming responder")]
    BecomingResponder,

    /// A general error that wraps other error types.
    ///
    /// This variant provides a way to include errors from other parts of
    /// the system (like I/O errors, serialization errors, etc.) within
    /// the mesh error hierarchy.
    #[error("error {0}")]
    Other(#[from] anyhow::Error),
}
