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
//! HKDF-SHA256 to derive independent directional transport keys, confirmation
//! keys, and a dedicated resume-authentication key:
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
use hmac::{Hmac, Mac};
use sha2::Sha256;
use spake2::{Ed25519Group, Identity, Password, Spake2};
use subtle::ConstantTimeEq;

/// Bytes the AEAD appends to every ciphertext.
pub const TAG_LEN: usize = 16;
/// Length of a key-confirmation MAC on the wire (raw bytes, sent as hex).
pub const CONFIRM_LEN: usize = 32;
/// Length of each fresh nonce in the resume challenge exchange.
pub const RESUME_NONCE_LEN: usize = 32;

/// Domain separator so a SPAKE2 transcript from this protocol can never be
/// replayed into a different one that happens to share a PIN.
const PAKE_IDENTITY: &[u8] = b"qpwgraph-rs/relay/v3";
const HKDF_INFO_CONTROL_C2H: &[u8] = b"qpw-relay control client->host";
const HKDF_INFO_CONTROL_H2C: &[u8] = b"qpw-relay control host->client";
const HKDF_INFO_AUDIO_C2H: &[u8] = b"qpw-relay audio client->host";
const HKDF_INFO_AUDIO_H2C: &[u8] = b"qpw-relay audio host->client";
const HKDF_INFO_CONFIRM_C: &[u8] = b"qpw-relay confirm client";
const HKDF_INFO_CONFIRM_H: &[u8] = b"qpw-relay confirm host";
const HKDF_INFO_RESUME_AUTH: &[u8] = b"qpw-relay resume authentication v1";
const HKDF_INFO_RESUME_CONTROL_C2H: &[u8] = b"qpw-relay resume control client->host v1";
const HKDF_INFO_RESUME_CONTROL_H2C: &[u8] = b"qpw-relay resume control host->client v1";
const RESUME_PROOF_DOMAIN: &[u8] = b"qpw-relay resume proof v1";
const RESUME_CONTROL_DOMAIN: &[u8] = b"qpw-relay resume control context v1";
const TRUSTED_PROOF_DOMAIN: &[u8] = b"qpw-relay trusted proof v1";
const TRUSTED_KEYS_DOMAIN: &[u8] = b"qpw-relay trusted session keys v1";

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
    /// Dedicated resume authentication material. It is never used directly
    /// as a control or audio key and is never sent on the wire.
    resume_auth: [u8; 32],
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
        let resume_auth = expand(HKDF_INFO_RESUME_AUTH);
        match side {
            Side::Client => Self {
                control_send: control_c2h,
                control_recv: control_h2c,
                audio_send: audio_c2h,
                audio_recv: audio_h2c,
                confirm_send: confirm_c,
                confirm_recv: confirm_h,
                resume_auth,
                side,
            },
            Side::Host => Self {
                control_send: control_h2c,
                control_recv: control_c2h,
                audio_send: audio_h2c,
                audio_recv: audio_c2h,
                confirm_send: confirm_h,
                confirm_recv: confirm_c,
                resume_auth,
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

    /// Dedicated key for authenticating a resume challenge. This key is
    /// derived from the original PAKE and is independent of every transport
    /// encryption key.
    pub(crate) fn resume_auth_key(&self) -> [u8; 32] {
        self.resume_auth
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

/// Compute the proof for one server challenge. The session id, protocol
/// version, generation, and both nonces are all bound to the MAC so a proof
/// cannot be moved to another session, challenge, or protocol context.
pub(crate) fn resume_proof(
    secret: &[u8; 32],
    session_id: u64,
    client_nonce: &[u8; RESUME_NONCE_LEN],
    server_nonce: &[u8; RESUME_NONCE_LEN],
    generation: u64,
) -> [u8; CONFIRM_LEN] {
    let mut message = Vec::with_capacity(
        RESUME_PROOF_DOMAIN.len() + 1 + 8 + 8 + RESUME_NONCE_LEN + RESUME_NONCE_LEN,
    );
    message.extend_from_slice(RESUME_PROOF_DOMAIN);
    message.push(crate::protocol::PROTOCOL_VERSION);
    message.extend_from_slice(&session_id.to_le_bytes());
    message.extend_from_slice(&generation.to_le_bytes());
    message.extend_from_slice(client_nonce);
    message.extend_from_slice(server_nonce);
    let mut mac =
        <Hmac<Sha256> as Mac>::new_from_slice(secret).expect("HMAC accepts a 32-byte key");
    mac.update(&message);
    mac.finalize().into_bytes().into()
}

/// Constant-time verification of a resume proof received from the peer.
pub(crate) fn verify_resume_proof(
    secret: &[u8; 32],
    session_id: u64,
    client_nonce: &[u8; RESUME_NONCE_LEN],
    server_nonce: &[u8; RESUME_NONCE_LEN],
    generation: u64,
    provided: &[u8],
) -> bool {
    if provided.len() != CONFIRM_LEN {
        return false;
    }
    resume_proof(secret, session_id, client_nonce, server_nonce, generation)
        .ct_eq(provided)
        .into()
}

/// Derive a brand-new pair of directional control channels for a successful
/// resume. The original audio/control keys are deliberately not reused, and
/// the nonce counters therefore start at zero only under these new keys.
pub(crate) fn resume_control_channel(
    secret: &[u8; 32],
    side: Side,
    session_id: u64,
    client_nonce: &[u8; RESUME_NONCE_LEN],
    server_nonce: &[u8; RESUME_NONCE_LEN],
    generation: u64,
) -> Result<(Sealer, Opener), RelayError> {
    let mut context = Vec::with_capacity(
        RESUME_CONTROL_DOMAIN.len() + 1 + 8 + 8 + RESUME_NONCE_LEN + RESUME_NONCE_LEN,
    );
    context.extend_from_slice(RESUME_CONTROL_DOMAIN);
    context.push(crate::protocol::PROTOCOL_VERSION);
    context.extend_from_slice(&session_id.to_le_bytes());
    context.extend_from_slice(&generation.to_le_bytes());
    context.extend_from_slice(client_nonce);
    context.extend_from_slice(server_nonce);

    let hkdf = Hkdf::<Sha256>::new(Some(&context), secret);
    let expand = |info: &[u8]| {
        let mut key = [0u8; 32];
        hkdf.expand(info, &mut key)
            .expect("HKDF-SHA256 always produces 32 bytes");
        key
    };
    let client_to_host = expand(HKDF_INFO_RESUME_CONTROL_C2H);
    let host_to_client = expand(HKDF_INFO_RESUME_CONTROL_H2C);
    match side {
        Side::Client => Ok((
            Sealer::new(&client_to_host, Side::Client)?,
            Opener::new(&host_to_client, Side::Host)?,
        )),
        Side::Host => Ok((
            Sealer::new(&host_to_client, Side::Host)?,
            Opener::new(&client_to_host, Side::Client)?,
        )),
    }
}

/// Authenticate a trusted reconnect challenge. The stable client and host
/// identities are included so a stored bearer credential cannot be replayed
/// against a different installation.
pub(crate) fn trusted_proof(
    secret: &[u8; 32],
    client_id: &str,
    host_id: &str,
    session_id: u64,
    client_nonce: &[u8; RESUME_NONCE_LEN],
    server_nonce: &[u8; RESUME_NONCE_LEN],
) -> [u8; CONFIRM_LEN] {
    let mut message = Vec::with_capacity(
        TRUSTED_PROOF_DOMAIN.len() + client_id.len() + host_id.len() + 2 * RESUME_NONCE_LEN + 2,
    );
    message.extend_from_slice(TRUSTED_PROOF_DOMAIN);
    message.extend_from_slice(&(client_id.len() as u16).to_le_bytes());
    message.extend_from_slice(client_id.as_bytes());
    message.extend_from_slice(&(host_id.len() as u16).to_le_bytes());
    message.extend_from_slice(host_id.as_bytes());
    message.extend_from_slice(&session_id.to_le_bytes());
    message.extend_from_slice(client_nonce);
    message.extend_from_slice(server_nonce);
    let mut mac =
        <Hmac<Sha256> as Mac>::new_from_slice(secret).expect("HMAC accepts a 32-byte key");
    mac.update(&message);
    mac.finalize().into_bytes().into()
}

pub(crate) fn verify_trusted_proof(
    secret: &[u8; 32],
    client_id: &str,
    host_id: &str,
    session_id: u64,
    client_nonce: &[u8; RESUME_NONCE_LEN],
    server_nonce: &[u8; RESUME_NONCE_LEN],
    provided: &[u8],
) -> bool {
    if provided.len() != CONFIRM_LEN {
        return false;
    }
    trusted_proof(
        secret,
        client_id,
        host_id,
        session_id,
        client_nonce,
        server_nonce,
    )
    .ct_eq(provided)
    .into()
}

/// Derive fresh directional channels for a trusted connection. The stored
/// credential is only the root; every connection still gets fresh nonce-bound
/// transport keys and a resume secret.
pub(crate) fn trusted_session_keys(
    secret: &[u8; 32],
    client_id: &str,
    host_id: &str,
    session_id: u64,
    client_nonce: &[u8; RESUME_NONCE_LEN],
    server_nonce: &[u8; RESUME_NONCE_LEN],
    side: Side,
) -> SessionKeys {
    let mut context = Vec::with_capacity(
        TRUSTED_KEYS_DOMAIN.len() + client_id.len() + host_id.len() + 2 * RESUME_NONCE_LEN + 2,
    );
    context.extend_from_slice(TRUSTED_KEYS_DOMAIN);
    context.extend_from_slice(&(client_id.len() as u16).to_le_bytes());
    context.extend_from_slice(client_id.as_bytes());
    context.extend_from_slice(&(host_id.len() as u16).to_le_bytes());
    context.extend_from_slice(host_id.as_bytes());
    context.extend_from_slice(&session_id.to_le_bytes());
    context.extend_from_slice(client_nonce);
    context.extend_from_slice(server_nonce);
    SessionKeys::derive(secret, &context, side)
}

/// Proofs for the authenticated secondary TCP audio connection used by ADB.
/// `side` is included in the domain so the two peers' responses cannot be
/// reflected into one another.
pub(crate) fn tcp_audio_proof(
    secret: &[u8; 32],
    session_id: u64,
    client_nonce: &[u8; RESUME_NONCE_LEN],
    server_nonce: &[u8; RESUME_NONCE_LEN],
    side: Side,
) -> [u8; CONFIRM_LEN] {
    let mut message = Vec::with_capacity(64);
    message.extend_from_slice(b"qpw-relay tcp audio proof v1");
    message.push(match side {
        Side::Client => 0,
        Side::Host => 1,
    });
    message.extend_from_slice(&session_id.to_le_bytes());
    message.extend_from_slice(client_nonce);
    message.extend_from_slice(server_nonce);
    let mut mac =
        <Hmac<Sha256> as Mac>::new_from_slice(secret).expect("HMAC accepts a 32-byte key");
    mac.update(&message);
    mac.finalize().into_bytes().into()
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

    fn resume_inputs() -> ([u8; 32], [u8; RESUME_NONCE_LEN], [u8; RESUME_NONCE_LEN]) {
        let (client, host) = paired("000000", "000000");
        assert_eq!(client.resume_auth_key(), host.resume_auth_key());
        (
            client.resume_auth_key(),
            [0x11; RESUME_NONCE_LEN],
            [0x22; RESUME_NONCE_LEN],
        )
    }

    #[test]
    fn resume_proof_requires_the_original_session_and_challenge() {
        let (secret, client_nonce, server_nonce) = resume_inputs();
        let proof = resume_proof(&secret, 7, &client_nonce, &server_nonce, 2);
        assert!(verify_resume_proof(
            &secret,
            7,
            &client_nonce,
            &server_nonce,
            2,
            &proof,
        ));

        let wrong_secret = [0x44; 32];
        assert!(!verify_resume_proof(
            &wrong_secret,
            7,
            &client_nonce,
            &server_nonce,
            2,
            &proof,
        ));
        assert!(!verify_resume_proof(
            &secret,
            8,
            &client_nonce,
            &server_nonce,
            2,
            &proof,
        ));
        assert!(!verify_resume_proof(
            &secret,
            7,
            &client_nonce,
            &server_nonce,
            3,
            &proof,
        ));

        let (other_client, _) = paired("000000", "000000");
        let other_secret = other_client.resume_auth_key();
        assert!(!verify_resume_proof(
            &other_secret,
            7,
            &client_nonce,
            &server_nonce,
            2,
            &proof,
        ));

        // A proof is challenge- and generation-specific. A later resume can
        // never accept material captured from this challenge.
        assert!(!verify_resume_proof(
            &secret,
            7,
            &client_nonce,
            &server_nonce,
            3,
            &proof,
        ));
        assert!(!verify_resume_proof(
            &secret,
            7,
            &client_nonce,
            &[0x33; RESUME_NONCE_LEN],
            2,
            &proof,
        ));
    }

    #[test]
    fn successful_resume_uses_fresh_control_keys() {
        let (secret, client_nonce, server_nonce) = resume_inputs();
        let (mut client_seal, _) =
            resume_control_channel(&secret, Side::Client, 7, &client_nonce, &server_nonce, 2)
                .unwrap();
        let (_, mut host_open) =
            resume_control_channel(&secret, Side::Host, 7, &client_nonce, &server_nonce, 2)
                .unwrap();
        let resumed = client_seal.seal(b"resume", b"header").unwrap();
        assert_eq!(
            host_open.open_sequential(&resumed, b"header").unwrap(),
            b"resume"
        );

        let (old_client, old_host) = paired("000000", "000000");
        let (mut old_seal, _) = old_client.control_channel().unwrap();
        let (_, mut old_open) = old_host.control_channel().unwrap();
        let old_frame = old_seal.seal(b"old", b"header").unwrap();
        assert!(host_open.open_sequential(&old_frame, b"header").is_err());
        assert!(old_open.open_sequential(&old_frame, b"header").is_ok());

        let (mut next_client, _) = resume_control_channel(
            &secret,
            Side::Client,
            7,
            &client_nonce,
            &[0x33; RESUME_NONCE_LEN],
            3,
        )
        .unwrap();
        assert_eq!(next_client.next_counter(), 0);
        let next_frame = next_client.seal(b"next", b"header").unwrap();
        assert!(host_open.open_sequential(&next_frame, b"header").is_err());
    }

    #[test]
    fn trusted_proofs_are_bound_to_both_installations_and_the_challenge() {
        let secret = [0x5a; 32];
        let client_nonce = [0x11; RESUME_NONCE_LEN];
        let server_nonce = [0x22; RESUME_NONCE_LEN];
        let proof = trusted_proof(
            &secret,
            "client-installation",
            "host-installation",
            41,
            &client_nonce,
            &server_nonce,
        );

        assert!(verify_trusted_proof(
            &secret,
            "client-installation",
            "host-installation",
            41,
            &client_nonce,
            &server_nonce,
            &proof,
        ));
        assert!(!verify_trusted_proof(
            &secret,
            "another-client",
            "host-installation",
            41,
            &client_nonce,
            &server_nonce,
            &proof,
        ));
        assert!(!verify_trusted_proof(
            &secret,
            "client-installation",
            "another-host",
            41,
            &client_nonce,
            &server_nonce,
            &proof,
        ));
        assert!(!verify_trusted_proof(
            &secret,
            "client-installation",
            "host-installation",
            42,
            &client_nonce,
            &server_nonce,
            &proof,
        ));
        assert!(!verify_trusted_proof(
            &secret,
            "client-installation",
            "host-installation",
            41,
            &client_nonce,
            &[0x33; RESUME_NONCE_LEN],
            &proof,
        ));
        assert!(!verify_trusted_proof(
            &[0x6b; 32],
            "client-installation",
            "host-installation",
            41,
            &client_nonce,
            &server_nonce,
            &proof,
        ));
        assert!(!verify_trusted_proof(
            &secret,
            "client-installation",
            "host-installation",
            41,
            &client_nonce,
            &server_nonce,
            &proof[..proof.len() - 1],
        ));
    }

    #[test]
    fn trusted_connections_derive_matching_fresh_directional_keys() {
        let secret = [0x5a; 32];
        let client_nonce = [0x11; RESUME_NONCE_LEN];
        let server_nonce = [0x22; RESUME_NONCE_LEN];
        let client = trusted_session_keys(
            &secret,
            "client-installation",
            "host-installation",
            41,
            &client_nonce,
            &server_nonce,
            Side::Client,
        );
        let host = trusted_session_keys(
            &secret,
            "client-installation",
            "host-installation",
            41,
            &client_nonce,
            &server_nonce,
            Side::Host,
        );
        assert_eq!(client.resume_auth_key(), host.resume_auth_key());
        assert!(host.verify_confirmation(&client.confirmation()));
        assert!(client.verify_confirmation(&host.confirmation()));

        let (mut client_seal, _) = client.control_channel().unwrap();
        let (_, mut host_open) = host.control_channel().unwrap();
        let frame = client_seal.seal(b"trusted", b"header").unwrap();
        assert_eq!(
            host_open.open_sequential(&frame, b"header").unwrap(),
            b"trusted"
        );

        let next = trusted_session_keys(
            &secret,
            "client-installation",
            "host-installation",
            42,
            &client_nonce,
            &server_nonce,
            Side::Client,
        );
        let (mut next_seal, _) = next.control_channel().unwrap();
        let next_frame = next_seal.seal(b"new session", b"header").unwrap();
        assert!(host_open.open_sequential(&next_frame, b"header").is_err());
    }
}
