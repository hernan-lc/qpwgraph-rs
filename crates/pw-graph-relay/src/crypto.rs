//! Pairing key agreement and authenticated encryption for relay sessions.
//!
//! # Why a PAKE instead of an HMAC over the PIN
//!
//! The pairing secret is a short numeric PIN a human reads off a screen.
//! A challenge/response of the shape `HMAC(key = PIN, msg = salt)` lets
//! anyone who observes a *single* exchange test every possible PIN offline —
//! a six-digit space is a million guesses, which is seconds of work. Constant
//! time comparison does not help: the attacker never talks to us again.
//!
//! [SPAKE2] fixes exactly that. Both sides mix the PIN into a Diffie-Hellman
//! exchange, and a transcript observer learns nothing that lets them test a
//! candidate PIN without talking to the host again. Guessing therefore costs
//! one *online* attempt per try, which [`crate::session`] rate limits.
//!
//! # What the exchange produces
//!
//! The SPAKE2 output plus the handshake transcript are run through
//! HKDF-SHA256 to derive four independent keys:
//!
//! - two directional ChaCha20-Poly1305 keys for the TCP control channel,
//! - two directional ChaCha20-Poly1305 keys for the UDP audio channel.
//!
//! Control frames use a strictly sequential nonce counter (TCP is ordered, so
//! a gap is an attack or a bug). Audio datagrams carry their nonce counter in
//! the cleartext header and are checked against a sliding replay window,
//! because UDP legitimately reorders and drops.
//!
//! Nothing in the relay accepts an unauthenticated packet: an attacker who
//! can reach the audio port but does not hold the session key cannot inject
//! audio, and — critically — cannot move the peer's audio address either.
//!
//! [SPAKE2]: https://datatracker.ietf.org/doc/html/draft-irtf-cfrg-spake2

use crate::RelayError;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use sha2::Sha256;
use spake2::{Ed25519Group, Identity, Password, Spake2};
use subtle::ConstantTimeEq;

/// Bytes the AEAD appends to every ciphertext.
pub const TAG_LEN: usize = 16;
/// Length of a key-confirmation MAC on the wire (raw bytes, sent as hex).
pub const CONFIRM_LEN: usize = 32;

/// Domain separator so a SPAKE2 transcript from this protocol can never be
/// replayed into a different one that happens to share a PIN.
const PAKE_IDENTITY: &[u8] = b"qpwgraph-rs/relay/v2";
const HKDF_INFO_CONTROL_C2H: &[u8] = b"qpw-relay control client->host";
const HKDF_INFO_CONTROL_H2C: &[u8] = b"qpw-relay control host->client";
const HKDF_INFO_AUDIO_C2H: &[u8] = b"qpw-relay audio client->host";
const HKDF_INFO_AUDIO_H2C: &[u8] = b"qpw-relay audio host->client";
const HKDF_INFO_CONFIRM_C: &[u8] = b"qpw-relay confirm client";
const HKDF_INFO_CONFIRM_H: &[u8] = b"qpw-relay confirm host";

/// Which end of the session a key set belongs to. Every derived key is
/// directional, so a reflected packet never decrypts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Client,
    Host,
}

impl Side {
    fn peer(self) -> Self {
        match self {
            Self::Client => Self::Host,
            Self::Host => Self::Client,
        }
    }
}

/// One side's half of a SPAKE2 exchange, waiting for the peer's message.
pub struct PakeStart {
    state: Spake2<Ed25519Group>,
    /// The message to hand the peer.
    pub message: Vec<u8>,
    side: Side,
}

/// Begin a pairing exchange for `pin`. The returned message is public and is
/// sent to the peer verbatim; it reveals nothing about the PIN.
pub fn pake_start(side: Side, pin: &str) -> PakeStart {
    let (state, message) = Spake2::<Ed25519Group>::start_symmetric(
        &Password::new(pin.as_bytes()),
        &Identity::new(PAKE_IDENTITY),
    );
    PakeStart {
        state,
        message,
        side,
    }
}

impl PakeStart {
    /// Complete the exchange with the peer's message and derive session keys.
    ///
    /// Fails when the peer's message is malformed. A *wrong PIN* does not
    /// fail here — SPAKE2 yields a different key on each side instead, and
    /// the mismatch is caught by key confirmation.
    pub fn finish(self, peer_message: &[u8]) -> Result<SessionKeys, RelayError> {
        let shared = self
            .state
            .finish(peer_message)
            .map_err(|error| RelayError::Protocol(format!("pairing exchange failed: {error}")))?;
        // The transcript is ordered client-first on both sides so the two
        // ends derive identical keys from identical input.
        let (client_message, host_message) = match self.side {
            Side::Client => (self.message.as_slice(), peer_message),
            Side::Host => (peer_message, self.message.as_slice()),
        };
        let mut transcript = Vec::with_capacity(client_message.len() + host_message.len());
        transcript.extend_from_slice(client_message);
        transcript.extend_from_slice(host_message);
        Ok(SessionKeys::derive(&shared, &transcript, self.side))
    }
}

/// The keys both ends derive from a completed pairing exchange.
pub struct SessionKeys {
    /// This side's view: keys are named by role, not by direction, so the
    /// same struct serves host and client.
    control_send: [u8; 32],
    control_recv: [u8; 32],
    audio_send: [u8; 32],
    audio_recv: [u8; 32],
    confirm_send: [u8; CONFIRM_LEN],
    confirm_recv: [u8; CONFIRM_LEN],
    side: Side,
}

impl SessionKeys {
    fn derive(shared: &[u8], transcript: &[u8], side: Side) -> Self {
        let hkdf = Hkdf::<Sha256>::new(Some(transcript), shared);
        let expand = |info: &[u8]| {
            let mut key = [0u8; 32];
            hkdf.expand(info, &mut key)
                .expect("HKDF-SHA256 always produces 32 bytes");
            key
        };
        let control_c2h = expand(HKDF_INFO_CONTROL_C2H);
        let control_h2c = expand(HKDF_INFO_CONTROL_H2C);
        let audio_c2h = expand(HKDF_INFO_AUDIO_C2H);
        let audio_h2c = expand(HKDF_INFO_AUDIO_H2C);
        let confirm_c = expand(HKDF_INFO_CONFIRM_C);
        let confirm_h = expand(HKDF_INFO_CONFIRM_H);
        match side {
            Side::Client => Self {
                control_send: control_c2h,
                control_recv: control_h2c,
                audio_send: audio_c2h,
                audio_recv: audio_h2c,
                confirm_send: confirm_c,
                confirm_recv: confirm_h,
                side,
            },
            Side::Host => Self {
                control_send: control_h2c,
                control_recv: control_c2h,
                audio_send: audio_h2c,
                audio_recv: audio_c2h,
                confirm_send: confirm_h,
                confirm_recv: confirm_c,
                side,
            },
        }
    }

    pub fn side(&self) -> Side {
        self.side
    }

    /// The confirmation value this side sends. Proving knowledge of a key
    /// derived from the PIN is what turns a successful SPAKE2 run into an
    /// authenticated one; without it a wrong PIN would only be noticed as
    /// garbled traffic later.
    pub fn confirmation(&self) -> [u8; CONFIRM_LEN] {
        self.confirm_send
    }

    /// Constant-time check of the peer's confirmation value.
    pub fn verify_confirmation(&self, provided: &[u8]) -> bool {
        if provided.len() != CONFIRM_LEN {
            return false;
        }
        self.confirm_recv.ct_eq(provided).into()
    }

    /// Sealer/opener pair for this side's control channel.
    pub fn control_channel(&self) -> Result<(Sealer, Opener), RelayError> {
        Ok((
            Sealer::new(&self.control_send, self.side)?,
            Opener::new(&self.control_recv, self.side.peer())?,
        ))
    }

    /// Sealer/opener pair for this side's audio channel.
    pub fn audio_channel(&self) -> Result<(Sealer, Opener), RelayError> {
        Ok((
            Sealer::new(&self.audio_send, self.side)?,
            Opener::new(&self.audio_recv, self.side.peer())?,
        ))
    }
}

/// Nonce prefix keeping the two directions in separate nonce spaces even if
/// a key were ever reused by mistake.
fn nonce_prefix(side: Side) -> [u8; 4] {
    match side {
        Side::Client => *b"QPWc",
        Side::Host => *b"QPWh",
    }
}

fn nonce_for(side: Side, counter: u64) -> Nonce {
    let mut bytes = [0u8; 12];
    bytes[..4].copy_from_slice(&nonce_prefix(side));
    bytes[4..].copy_from_slice(&counter.to_le_bytes());
    *Nonce::from_slice(&bytes)
}

fn cipher(key: &[u8; 32]) -> Result<ChaCha20Poly1305, RelayError> {
    ChaCha20Poly1305::new_from_slice(Key::from_slice(key))
        .map_err(|error| RelayError::Protocol(format!("session key rejected: {error}")))
}

/// Encrypts outbound frames with an increasing nonce counter.
pub struct Sealer {
    cipher: ChaCha20Poly1305,
    side: Side,
    counter: u64,
}

impl Sealer {
    fn new(key: &[u8; 32], side: Side) -> Result<Self, RelayError> {
        Ok(Self {
            cipher: cipher(key)?,
            side,
            counter: 0,
        })
    }

    /// Counter that [`Self::seal`] will use next. Audio senders copy it into
    /// the packet header so the receiver can reconstruct the nonce.
    pub fn next_counter(&self) -> u64 {
        self.counter
    }

    /// Encrypt `plaintext`, authenticating `aad` alongside it.
    pub fn seal(&mut self, plaintext: &[u8], aad: &[u8]) -> Result<Vec<u8>, RelayError> {
        let nonce = nonce_for(self.side, self.counter);
        let sealed = self
            .cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: plaintext,
                    aad,
                },
            )
            .map_err(|_| RelayError::Protocol("frame encryption failed".into()))?;
        // 2^64 frames is unreachable at any real frame rate, so a plain
        // increment cannot wrap into nonce reuse.
        self.counter += 1;
        Ok(sealed)
    }
}

/// Decrypts inbound frames, rejecting replays.
pub struct Opener {
    cipher: ChaCha20Poly1305,
    side: Side,
    /// Highest counter accepted so far; the window is relative to it.
    highest: u64,
    /// Bitmap of the [`REPLAY_WINDOW`] counters below `highest`.
    seen: u64,
    started: bool,
}

/// Counters below the highest accepted one that are still acceptable. UDP
/// reorders, but not by thousands of packets; anything older is either lost
/// beyond usefulness or a replay.
pub const REPLAY_WINDOW: u64 = 64;

impl Opener {
    fn new(key: &[u8; 32], side: Side) -> Result<Self, RelayError> {
        Ok(Self {
            cipher: cipher(key)?,
            side,
            highest: 0,
            seen: 0,
            started: false,
        })
    }

    /// Decrypt a control frame. The control channel runs over TCP, so the
    /// counter must be exactly the next one: a gap means tampering.
    pub fn open_sequential(&mut self, sealed: &[u8], aad: &[u8]) -> Result<Vec<u8>, RelayError> {
        let counter = if self.started { self.highest + 1 } else { 0 };
        let plaintext = self.decrypt(counter, sealed, aad)?;
        self.highest = counter;
        self.started = true;
        Ok(plaintext)
    }

    /// Decrypt a datagram whose nonce counter travelled in the clear header,
    /// rejecting replays and counters outside the window.
    pub fn open_windowed(
        &mut self,
        counter: u64,
        sealed: &[u8],
        aad: &[u8],
    ) -> Result<Vec<u8>, RelayError> {
        if self.started && !self.window_allows(counter) {
            return Err(RelayError::Protocol(
                "audio packet replayed or too old".into(),
            ));
        }
        let plaintext = self.decrypt(counter, sealed, aad)?;
        self.record(counter);
        Ok(plaintext)
    }

    fn window_allows(&self, counter: u64) -> bool {
        if counter > self.highest {
            return true;
        }
        let distance = self.highest - counter;
        if distance >= REPLAY_WINDOW {
            return false;
        }
        self.seen & (1u64 << distance) == 0
    }

    fn record(&mut self, counter: u64) {
        if !self.started {
            self.highest = counter;
            self.seen = 1;
            self.started = true;
            return;
        }
        if counter > self.highest {
            let shift = counter - self.highest;
            self.seen = if shift >= REPLAY_WINDOW {
                1
            } else {
                (self.seen << shift) | 1
            };
            self.highest = counter;
        } else {
            let distance = self.highest - counter;
            if distance < REPLAY_WINDOW {
                self.seen |= 1u64 << distance;
            }
        }
    }

    fn decrypt(&self, counter: u64, sealed: &[u8], aad: &[u8]) -> Result<Vec<u8>, RelayError> {
        let nonce = nonce_for(self.side, counter);
        self.cipher
            .decrypt(&nonce, Payload { msg: sealed, aad })
            .map_err(|_| RelayError::Protocol("frame authentication failed".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paired(pin_client: &str, pin_host: &str) -> (SessionKeys, SessionKeys) {
        let client = pake_start(Side::Client, pin_client);
        let host = pake_start(Side::Host, pin_host);
        let client_message = client.message.clone();
        let host_message = host.message.clone();
        (
            client.finish(&host_message).expect("client finishes"),
            host.finish(&client_message).expect("host finishes"),
        )
    }

    #[test]
    fn matching_pins_agree_on_every_key() {
        let (client, host) = paired("123456", "123456");
        assert!(host.verify_confirmation(&client.confirmation()));
        assert!(client.verify_confirmation(&host.confirmation()));

        let (mut client_seal, _) = client.control_channel().unwrap();
        let (_, mut host_open) = host.control_channel().unwrap();
        let sealed = client_seal.seal(b"hello", b"header").unwrap();
        assert_eq!(
            host_open.open_sequential(&sealed, b"header").unwrap(),
            b"hello"
        );
    }

    #[test]
    fn a_wrong_pin_fails_key_confirmation() {
        let (client, host) = paired("123456", "654321");
        assert!(!host.verify_confirmation(&client.confirmation()));
        assert!(!client.verify_confirmation(&host.confirmation()));
    }

    #[test]
    fn a_transcript_reveals_nothing_reusable() {
        // Two runs with the same PIN produce unrelated keys, so capturing one
        // exchange cannot be replayed against the next.
        let (first, _) = paired("123456", "123456");
        let (second, _) = paired("123456", "123456");
        assert_ne!(first.confirmation(), second.confirmation());
    }

    #[test]
    fn tampering_with_the_aad_or_ciphertext_is_detected() {
        let (client, host) = paired("000000", "000000");
        let (mut seal, _) = client.audio_channel().unwrap();
        let (_, mut open) = host.audio_channel().unwrap();
        let counter = seal.next_counter();
        let sealed = seal.seal(b"audio frame", b"header").unwrap();
        assert!(open
            .open_windowed(counter, &sealed, b"other header")
            .is_err());
        let mut broken = sealed.clone();
        broken[0] ^= 0x01;
        assert!(open.open_windowed(counter, &broken, b"header").is_err());
        assert!(open.open_windowed(counter, &sealed, b"header").is_ok());
    }

    #[test]
    fn replayed_and_ancient_datagrams_are_refused() {
        let (client, host) = paired("000000", "000000");
        let (mut seal, _) = client.audio_channel().unwrap();
        let (_, mut open) = host.audio_channel().unwrap();
        let mut sealed = Vec::new();
        for _ in 0..4 {
            let counter = seal.next_counter();
            sealed.push((counter, seal.seal(b"frame", b"h").unwrap()));
        }
        // Out-of-order delivery inside the window is fine.
        assert!(open.open_windowed(sealed[2].0, &sealed[2].1, b"h").is_ok());
        assert!(open.open_windowed(sealed[0].0, &sealed[0].1, b"h").is_ok());
        // The same datagram twice is not.
        assert!(open.open_windowed(sealed[0].0, &sealed[0].1, b"h").is_err());
    }

    #[test]
    fn a_control_gap_is_refused() {
        let (client, host) = paired("000000", "000000");
        let (mut seal, _) = client.control_channel().unwrap();
        let (_, mut open) = host.control_channel().unwrap();
        let first = seal.seal(b"one", b"").unwrap();
        let second = seal.seal(b"two", b"").unwrap();
        // Skipping the first frame must not decrypt as the second.
        assert!(open.open_sequential(&second, b"").is_err());
        assert!(open.open_sequential(&first, b"").is_ok());
        assert!(open.open_sequential(&second, b"").is_ok());
    }
}
