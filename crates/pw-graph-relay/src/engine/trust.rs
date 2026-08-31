//! Trusted peers and the enrollment handshake that creates them.
//!
//! A credential is only stored once both sides have acknowledged it, so an
//! enrollment is a transaction with an explicit decision and resolution
//! rather than a single write.

use super::*;

/// Pairing failures recorded against one source address.
pub(crate) struct FailureRecord {
    pub(crate) count: u32,
    pub(crate) locked_until: Instant,
    pub(crate) last_seen: Instant,
}

pub(crate) enum EnrollmentDecision {
    Pending,
    Accepted,
    Rejected(String),
}

pub(crate) struct PendingEnrollment {
    pub(crate) session_id: SessionId,
    pub(crate) peer_id: String,
    pub(crate) secret: [u8; 32],
    pub(crate) created: Instant,
    pub(crate) decision: EnrollmentDecision,
}

pub(crate) struct EnrollmentResolution {
    pub(crate) peer_id: String,
    pub(crate) secret: [u8; 32],
    pub(crate) accepted: bool,
    pub(crate) reason: Option<String>,
}

/// A persistent bearer credential for one authenticated peer.
///
/// The engine never derives this from a PIN. It is generated after an
/// explicit PIN pairing and can therefore be used for later cable/network
/// discovery without weakening the one-time pairing exchange.
#[derive(Clone, PartialEq, Eq)]
pub struct TrustedPeer {
    pub peer_id: String,
    pub secret: [u8; 32],
}

impl fmt::Debug for TrustedPeer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TrustedPeer")
            .field("peer_id", &self.peer_id)
            .field("secret", &"<redacted>")
            .finish()
    }
}
