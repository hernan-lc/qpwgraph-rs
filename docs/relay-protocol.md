# Relay wire protocol, version 2

The relay carries audio between this machine and a peer (typically a phone)
over the local network. It uses two channels:

- a **control channel** over TCP, carrying length-prefixed JSON frames,
- an **audio channel** over UDP, carrying one encoded codec frame per
  datagram.

Version 2 replaced version 1 wholesale. Version 1 is not accepted and there is
no downgrade path, because the changes are the security model: version 1
authenticated only the TCP handshake, and its UDP audio channel had no session
identifier, no MAC, and no encryption at all. Anyone who could reach the audio
port could inject audio into a session and — because the host adopted the
source address of any syntactically valid datagram — redirect the session's
outbound audio to themselves, without ever knowing the PIN.

## Threat model

The relay is designed to be safe on a network containing untrusted devices:
a shared Wi-Fi network, a conference LAN, a phone tether. An attacker is
assumed to be able to see, modify, drop, and inject packets on both channels,
and to know the host's addresses and ports.

What the protocol guarantees against such an attacker:

- **They cannot join a session.** Pairing is a PAKE; without the PIN they
  cannot complete it, and observing any number of attempts does not let them
  test PIN guesses offline.
- **They cannot read or forge traffic.** Both channels are encrypted and
  authenticated with ChaCha20-Poly1305 under keys derived from the pairing.
- **They cannot replay traffic.** The control channel requires strictly
  sequential nonces; the audio channel enforces a sliding replay window.
- **They cannot redirect a session's audio.** The peer's audio address is
  updated only from a datagram that authenticated under the session key.

What it does not defend against: an attacker who learns the PIN, and denial of
service by flooding (the connection and handshake limits bound the cost, they
do not eliminate it).

## Pairing

The host displays a six-digit PIN. Both ends run a symmetric
[SPAKE2](https://datatracker.ietf.org/doc/html/draft-irtf-cfrg-spake2)
exchange over the Ed25519 group with that PIN as the password and the
domain-separating identity `qpwgraph-rs/relay/v2`.

Six digits is a small space, which is exactly why the PIN is never used as a
raw key or MAC key. With a PAKE, an observer of the exchange learns nothing
that lets them test a candidate PIN; guessing requires a fresh online attempt
against the host, and the host allows five failures per source address before
locking it out for a minute.

```text
C → H  Hello        protocol, device_name, device_kind, roles,
                    sample_rate, channels, pake (client's SPAKE2 message, hex)
H → C  Challenge    protocol, host_name, pake (host's SPAKE2 message, hex)
C → H  Pair         confirm (client's key confirmation, hex)
H → C  PairConfirm  confirm (host's key confirmation, hex)
```

Both SPAKE2 messages are public; sending the client's in `Hello` keeps pairing
to two round trips.

Each side derives the shared SPAKE2 output, then runs HKDF-SHA256 over it with
the transcript (`client_message || host_message`, in that fixed order on both
sides) as salt to produce six independent 32-byte values:

| Info string                      | Purpose                            |
| -------------------------------- | ---------------------------------- |
| `qpw-relay control client->host` | Control channel, client to host    |
| `qpw-relay control host->client` | Control channel, host to client    |
| `qpw-relay audio client->host`   | Audio channel, client to host      |
| `qpw-relay audio host->client`   | Audio channel, host to client      |
| `qpw-relay confirm client`       | Client's key confirmation value    |
| `qpw-relay confirm host`         | Host's key confirmation value      |

The confirmation values are what turn a completed SPAKE2 run into an
*authenticated* one. A wrong PIN does not make SPAKE2 fail; it makes the two
sides derive different keys. Each end sends the confirmation value the other
end can independently compute and compares the received one in constant time.
A mismatch is a wrong PIN, and the host counts it against the source's
attempt budget.

Every frame after `PairConfirm` is sealed.

## Control channel

```text
magic "QPR2" (4 bytes) | version u8 = 2 | payload length u32 LE | payload
```

Before key confirmation the payload is JSON. After it, the payload is the
ChaCha20-Poly1305 sealing of the JSON, with the 9-byte header authenticated as
associated data.

Nonces are 12 bytes: a four-byte direction prefix (`QPWc` from the client,
`QPWh` from the host) followed by a little-endian 64-bit counter. TCP delivers
in order, so the receiver requires exactly the next counter — a gap is
tampering, not reordering.

Frames larger than 64 KiB are refused; these are small JSON documents.

Message types after pairing: `PairOk` (audio port and session id),
`SessionStart` / `SessionReady` (negotiated codec and geometry), `Keepalive`
(every 2 s, with a 6 s timeout), `ControlHint` (volume and mute hints),
`Resume` / `ResumeOk`, and `Bye`. An unrecognised `type` decodes as `Unknown`
rather than killing the connection, so a newer peer can add messages.

### Negotiated parameters

`SessionStart` proposes a codec (`pcm` or `opus`), a frame duration, a sample
rate, and a channel count. The host accepts only:

- frame duration: 5, 10, 20, 40, or 60 ms,
- sample rate: 16 000, 24 000, or 48 000 Hz,
- channels: 1 or 2.

These are the session's geometry, not the machine's. Each end converts between
the session geometry and its own local audio geometry, and mixes the sessions
only once they share a format — so two peers at different rates do not
interfere with each other, and a 16 kHz peer does not play back at three times
the pitch.

## Audio channel

```text
offset 0   u16  magic 0xA1E5
offset 2   u8   version (low nibble) = 2 | flags (high nibble)
                flag 0x10 = stereo, flag 0x20 = keyframe
offset 3   u8   codec id (0 = f32 LE PCM, 1 = Opus)
offset 4   u32  sequence number, one per frame
offset 8   u32  sender timestamp in milliseconds
offset 12  u64  AEAD nonce counter, strictly increasing per sender
offset 20  ..   ChaCha20-Poly1305 ciphertext, then its 16-byte tag
```

The 20-byte header is cleartext — the receiver needs the nonce counter before
it can decrypt — but is authenticated as associated data, so a single flipped
header bit makes the datagram fail to open.

A datagram that does not open is dropped immediately, before anything else
observes it: before the peer-address bookkeeping, before the jitter buffer,
before the decoder. That ordering is the fix for version 1's address-hijacking
flaw.

Nonce counters are checked against a 64-packet sliding window, so genuine
reordering is accepted and replays are not.

An **announce** packet has an empty plaintext (on the wire, a header and a bare
tag). A client sends one right after pairing, and again after a resume, so the
host learns its UDP source address. Because it is sealed with the session key,
only the paired client can move that address — which is what lets a session
survive the phone roaming from Wi-Fi to a USB tether.

A PCM payload must be exactly `frame_samples × 4` bytes. Short, long, and
ragged payloads are dropped rather than partially decoded: in a framed
realtime protocol a wrong-sized packet is a corrupt packet, and accepting one
only perturbs the stream's timing.

## Resume

If the control link drops, the client re-dials and sends `Resume` with the
session id and a fresh SPAKE2 message. The host answers with `Challenge`, and
the two ends run the full pairing exchange again — so the resumed control
channel gets **new** keys and a captured earlier transcript is useless against
it.

Audio keys are *not* rederived: the UDP workers never stopped, and their
nonce counters and replay windows carry on unbroken. The host holds a dropped
session open for 15 seconds; the client makes three attempts with exponential
backoff.

## Resource limits

Every limit below exists because the thing it bounds is reachable by an
unauthenticated peer.

| Limit                       | Default | What it bounds                            |
| --------------------------- | ------- | ----------------------------------------- |
| Concurrent handshakes       | 8       | Threads sitting in a pre-auth read timeout |
| Established sessions        | 16      | Per-session threads and buffers            |
| Pairing failures per source | 5       | Online PIN guessing, then a 60 s lockout   |
| Jitter buffer forward window| 64      | Frames a peer may queue ahead of playback  |
| Jitter buffer capacity      | 128     | Frames held regardless of sequence spread  |
| Control frame size          | 64 KiB  | Allocation per control frame               |
| Event queue                 | 256     | Events held for a slow UI consumer         |

The host binds `0.0.0.0` by default but honours a configured bind address, so
the relay can be confined to one link rather than offered on the LAN, every
VPN, and whatever else happens to be up.
