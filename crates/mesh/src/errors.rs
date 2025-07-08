use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("already connected to peer")]
    AlreadyConnected,
    #[error("becoming responder")]
    BecomingResponder,
    #[error("error {0}")]
    Other(#[from] anyhow::Error),
}
