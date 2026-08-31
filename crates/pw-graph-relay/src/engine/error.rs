//! What the engine can refuse to do.

use super::*;

#[derive(Debug, Error)]
pub enum RelayError {
    #[error("relay I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("relay protocol error: {0}")]
    Protocol(String),
    #[error("relay codec error: {0}")]
    Codec(String),
    #[error("relay engine error: {0}")]
    Engine(String),
    /// A caller-supplied configuration that could never work — an audio
    /// geometry outside the negotiable set, say. Distinguished from
    /// [`Self::Protocol`] because nothing was ever put on the wire: the
    /// mistake is local and the caller can fix it directly.
    #[error("relay configuration error: {0}")]
    Config(String),
}

pub type RelayResult<T> = Result<T, RelayError>;
