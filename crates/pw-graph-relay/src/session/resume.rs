//! Resuming a dropped session: the challenge/proof exchange, the grace period
//! a host holds the session open for, and the client's reconnect targets.

use super::*;

pub(super) fn fresh_resume_nonce() -> [u8; RESUME_NONCE_LEN] {
    let mut nonce = [0u8; RESUME_NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce);
    nonce
}

pub(super) fn decode_resume_nonce(value: &str) -> Option<[u8; RESUME_NONCE_LEN]> {
    hex_decode(value).ok()?.try_into().ok()
}

/// Host-side resume of a session whose control link dropped. A resume is
/// challenge-response authenticated with the secret derived during the
/// original PAKE; the host-wide PIN is deliberately not sufficient here.
/// Only the control channel is rekeyed: the UDP audio workers never stopped,
/// so their keys and replay windows carry on untouched.
pub(super) fn resume_peer_session(
    inner: &Arc<EngineInner>,
    id: SessionId,
    stream: TcpStream,
    client_nonce: &str,
) {
    resume_peer_session_with(inner, id, stream, client_nonce, &bind_audio_socket_at);
}

pub(super) fn resume_peer_session_with(
    inner: &Arc<EngineInner>,
    id: SessionId,
    mut stream: TcpStream,
    client_nonce: &str,
    bind: AudioBinder,
) {
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT));
    let peer_addr = stream.peer_addr().ok();

    let Some(record) = inner.session(id) else {
        let _ = write_frame(
            &mut stream,
            &ControlMessage::PairFail {
                reason: "unknown or expired session".into(),
            },
        );
        return;
    };

    let Some(client_nonce) = decode_resume_nonce(client_nonce) else {
        let _ = write_frame(
            &mut stream,
            &ControlMessage::PairFail {
                reason: "malformed resume nonce".into(),
            },
        );
        return;
    };

    // The old control owner must have actually dropped before this state can
    // be claimed. This also consumes the one in-flight challenge, so racing
    // reconnects and a proof replay cannot both take over the session.
    let Some(generation) = record.begin_resume() else {
        let _ = write_frame(
            &mut stream,
            &ControlMessage::PairFail {
                reason: "the session control channel is still active".into(),
            },
        );
        return;
    };

    let server_nonce = fresh_resume_nonce();
    if write_frame(
        &mut stream,
        &ControlMessage::ResumeChallenge {
            server_nonce: hex_encode(&server_nonce),
            generation,
        },
    )
    .is_err()
    {
        record.cancel_resume(generation);
        return;
    }

    let proof = match read_frame(&mut stream) {
        Ok(ControlMessage::ResumeProof { proof }) => hex_decode(&proof).ok(),
        _ => None,
    };
    let valid = proof
        .as_deref()
        .map(|proof| {
            verify_resume_proof(
                &record.resume_secret,
                record.wire_id,
                &client_nonce,
                &server_nonce,
                generation,
                proof,
            )
        })
        .unwrap_or(false);
    if !valid {
        record.cancel_resume(generation);
        let _ = write_frame(
            &mut stream,
            &ControlMessage::PairFail {
                reason: "resume authentication failed".into(),
            },
        );
        return;
    }

    if !inner.session_alive(record.id) {
        record.cancel_resume(generation);
        return;
    }

    let Ok((sealer, opener)) = resume_control_channel(
        &record.resume_secret,
        Side::Host,
        record.wire_id,
        &client_nonce,
        &server_nonce,
        generation,
    ) else {
        record.cancel_resume(generation);
        return;
    };
    let mut cipher = ControlCipher { sealer, opener };
    if let Some(peer_addr) = peer_addr {
        let migrated = migrate_udp_audio_socket_with(inner, &record, peer_addr, true, bind);
        if let Err(error) = migrated {
            inner.emit(RelayEvent::Error {
                message: format!("UDP audio interface migration failed: {error}"),
            });
            if error.is_fatal() {
                // The negotiated UDP port could not be reopened, so the
                // client's audio is going nowhere. Acknowledging this resume
                // would hand back a session with a live control link and a
                // permanently silent audio path, so reject it instead and let
                // the peer negotiate a fresh session. `PairFail` is already
                // what a resuming client expects on a rejected candidate, so
                // this needs no wire change.
                let reason = format!("relay audio endpoint could not be restored: {error}");
                let _ = cipher.send(
                    &mut stream,
                    &ControlMessage::PairFail {
                        reason: reason.clone(),
                    },
                );
                record.cancel_resume(generation);
                teardown(inner, id, reason);
                return;
            }
            // Otherwise the old socket is still installed and serving: a
            // failed rebind must not destroy a still-usable authenticated
            // session, and the next authenticated resume can retry.
        }
        // The control address is learned from the authenticated resume, never
        // from discovery. It is reported independently from the stable peer
        // identity so diagnostics show the path actually in use.
        if let Ok(mut current) = record.control_peer_addr.lock() {
            *current = peer_addr;
        }
    }
    // Commit the state transition before acknowledging success. The grace
    // watcher may win the deadline race while this worker is deriving keys;
    // in that case the old control owner has already been declared gone and
    // this challenge must not produce a false-positive ResumeOk.
    if !record.finish_resume(generation) {
        return;
    }
    if cipher
        .send(&mut stream, &ControlMessage::ResumeOk {})
        .is_err()
    {
        // The old grace watcher has exited after the generation transition.
        // Re-enter the normal grace state so a failed response does not leave
        // the session permanently marked Active without a control owner.
        // `finish_resume` already rotated the control generation, so the
        // original watcher has correctly returned `Resumed` and cannot watch
        // this new owner. Re-enter the eligible state explicitly and give one
        // bounded replacement attempt. If no replacement arrives, tear the
        // record down here instead of leaving an Active zombie in the map.
        handle_failed_resume_ok(inner, &record);
        return;
    }
    let inner = Arc::clone(inner);
    host_control_loop(inner, record, stream, cipher);
}

/// Wait for a client resume to replace this control watch. Returns `true`
/// when somebody else now owns the session (no teardown by the caller).
pub(super) fn await_resume_grace(inner: &Arc<EngineInner>, record: &Arc<SessionRecord>) -> bool {
    await_resume_grace_with_deadlines(inner, record, RESUME_GRACE, HANDSHAKE_TIMEOUT)
}

/// Recover from committing a new control generation before its `ResumeOk`
/// could be delivered. This is kept as one helper so the failure path cannot
/// accidentally leave an active-but-ownerless record in the session map.
pub(super) fn handle_failed_resume_ok(
    inner: &Arc<EngineInner>,
    record: &Arc<SessionRecord>,
) -> bool {
    handle_failed_resume_ok_with_deadlines(inner, record, RESUME_GRACE, HANDSHAKE_TIMEOUT)
}

pub(super) fn handle_failed_resume_ok_with_deadlines(
    inner: &Arc<EngineInner>,
    record: &Arc<SessionRecord>,
    grace: Duration,
    handshake_timeout: Duration,
) -> bool {
    let _ = record.mark_control_dropped();
    if await_resume_grace_with_deadlines(inner, record, grace, handshake_timeout) {
        true
    } else {
        teardown(
            inner,
            record.id,
            "resumed control channel could not deliver ResumeOk".into(),
        );
        false
    }
}

pub(super) fn await_resume_grace_with_deadlines(
    inner: &Arc<EngineInner>,
    record: &Arc<SessionRecord>,
    grace: Duration,
    handshake_timeout: Duration,
) -> bool {
    let _ = record.mark_control_dropped();
    let generation = record.control_generation.load(Ordering::Relaxed);
    let deadline = Instant::now() + grace;
    let mut in_flight_deadline = None;
    loop {
        if record.stop.load(Ordering::Relaxed) || !inner.session_alive(record.id) {
            return true;
        }
        if record.control_generation.load(Ordering::Acquire) != generation {
            return true;
        }
        if Instant::now() >= deadline {
            // Expiry and a successful resume serialize on the record state
            // lock. A stale watcher must not tear down a session after the
            // new control owner has completed its handshake.
            match record.expire_resume_grace(generation) {
                ResumeGraceResult::Expired => return false,
                ResumeGraceResult::Resumed => return true,
                ResumeGraceResult::InProgress { generation } => {
                    let challenge_deadline = *in_flight_deadline
                        .get_or_insert_with(|| Instant::now() + handshake_timeout);
                    if Instant::now() >= challenge_deadline && record.abort_resume(generation) {
                        return false;
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Re-dial the host and resume an established session. Returns the new
/// control stream and its freshly rekeyed cipher on success.
pub(super) fn resume_client_control(
    inner: &Arc<EngineInner>,
    record: &Arc<SessionRecord>,
    target: SocketAddr,
) -> Option<(TcpStream, ControlCipher, SocketAddr)> {
    let config = inner.config();
    let mut backoff = Duration::from_millis(500);
    for _ in 0..RESUME_ATTEMPTS {
        if !inner.session_alive(record.id) || record.stop.load(Ordering::Relaxed) {
            return None;
        }
        std::thread::sleep(backoff);
        backoff = (backoff * 2).min(Duration::from_secs(4));

        let targets = resume_targets(inner, record, target);
        let mut connected = None;
        for candidate in targets {
            if !inner.candidate_allowed(&record.peer.id, candidate) {
                continue;
            }
            let links = netlink::local_links();
            let bind = netlink::outbound_bind_addr(&links, candidate, config.transport);
            // Try with policy bind first; on No route to host (EHOSTUNREACH 113,
            // ENETUNREACH 101) retry same candidate with wildcard bind before
            // marking the candidate failed. This handles stale mDNS cache after
            // interface migration where the old address is still ranked but the
            // policy bind (e.g. USB source → Wi-Fi target) is unroutable.
            let stream = match connect_control_tcp(candidate, bind, config.transport) {
                Ok(s) => Ok(s),
                Err(e) if e.raw_os_error() == Some(113)
                    || e.raw_os_error() == Some(101)
                    || e.raw_os_error() == Some(99) =>
                {
                    connect_control_tcp(candidate, None, config.transport)
                }
                Err(e) => Err(e),
            };
            match stream {
                Ok(stream) => {
                    connected = Some((stream, candidate));
                    break;
                }
                Err(_) => inner.note_candidate_failure(&record.peer.id, candidate),
            }
        }
        let Some((mut stream, resumed_target)) = connected else {
            continue;
        };
        let _ = stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT));
        let client_nonce = fresh_resume_nonce();
        if write_frame(
            &mut stream,
            &ControlMessage::ResumeHello {
                session_id: record.wire_id,
                client_nonce: hex_encode(&client_nonce),
            },
        )
        .is_err()
        {
            inner.note_candidate_failure(&record.peer.id, resumed_target);
            continue;
        }
        let (server_nonce, generation) = match read_frame(&mut stream) {
            Ok(ControlMessage::ResumeChallenge {
                server_nonce,
                generation,
            }) => (server_nonce, generation),
            Ok(ControlMessage::PairFail { reason }) => {
                if reason == "the session control channel is still active" {
                    // A previous control watcher may not have observed the
                    // drop yet. Treat this as a retryable race, not as a
                    // terminal failure for the reconnecting client.
                    continue;
                }
                inner.note_candidate_failure(&record.peer.id, resumed_target);
                // PairFail is cleartext at this point. It is evidence only
                // about this address, not about the session-wide resume
                // secret or the peer identity. Continue with other bounded
                // candidates, including a real USB path behind a spoofed ID.
                let _ = reason;
                continue;
            }
            _ => {
                inner.note_candidate_failure(&record.peer.id, resumed_target);
                continue;
            }
        };
        let Some(server_nonce) = decode_resume_nonce(&server_nonce) else {
            inner.note_candidate_failure(&record.peer.id, resumed_target);
            continue;
        };
        let proof = resume_proof(
            &record.resume_secret,
            record.wire_id,
            &client_nonce,
            &server_nonce,
            generation,
        );
        if write_frame(
            &mut stream,
            &ControlMessage::ResumeProof {
                proof: hex_encode(&proof),
            },
        )
        .is_err()
        {
            inner.note_candidate_failure(&record.peer.id, resumed_target);
            continue;
        }
        let Ok((sealer, opener)) = resume_control_channel(
            &record.resume_secret,
            Side::Client,
            record.wire_id,
            &client_nonce,
            &server_nonce,
            generation,
        ) else {
            inner.note_candidate_failure(&record.peer.id, resumed_target);
            continue;
        };
        let mut cipher = ControlCipher { sealer, opener };
        match cipher.receive(&mut stream) {
            Ok(ControlMessage::ResumeOk {}) => {
                let _ = stream.set_read_timeout(Some(Duration::from_millis(250)));
                inner.note_candidate_success(&record.peer.id, resumed_target);
                return Some((stream, cipher, resumed_target));
            }
            Ok(ControlMessage::PairFail { reason }) => {
                inner.note_candidate_failure(&record.peer.id, resumed_target);
                // A rejection here is still candidate-local. Continue so a
                // legitimate address can complete the session's proof flow.
                let _ = reason;
                continue;
            }
            _ => {
                inner.note_candidate_failure(&record.peer.id, resumed_target);
                continue;
            }
        }
    }
    None
}

/// Return the original destination plus addresses discovered for the same
/// stable peer. Discovery is deliberately identity-scoped: a nearby host
/// appearing on USB must not become a resume target merely because its port
/// matches.
pub(super) fn resume_targets(
    inner: &Arc<EngineInner>,
    record: &Arc<SessionRecord>,
    original: SocketAddr,
) -> Vec<SocketAddr> {
    let mut targets = vec![original];
    for peer in inner.discovered_peers() {
        if peer.id == record.peer.id
            || (record.peer.id == record.peer.name && peer.name == record.peer.name)
        {
            let candidate = SocketAddr::new(peer.addr.ip(), original.port());
            if !targets.contains(&candidate) {
                targets.push(candidate);
            }
        }
    }
    targets.sort_by_key(|candidate| candidate_rank(inner, record, *candidate, original));
    // Discovery metadata is untrusted and may contain many addresses for a
    // forged stable ID. Keep reconnect work bounded while retaining the
    // original target (it is always included before the ranked truncation).
    targets.truncate(crate::MAX_TRUSTED_CANDIDATE_ADDRESSES);
    targets
}

pub(super) fn candidate_rank(
    inner: &Arc<EngineInner>,
    record: &Arc<SessionRecord>,
    candidate: SocketAddr,
    original: SocketAddr,
) -> (u8, SocketAddr) {
    if inner.last_successful_address(&record.peer.id) == Some(candidate) {
        return (0, candidate);
    }
    let links = netlink::local_links();
    let classified = inner.discovered_link(candidate).or_else(|| {
        let IpAddr::V4(address) = candidate.ip() else {
            return None;
        };
        links
            .iter()
            .find(|link| link.contains(address))
            .map(|link| link.kind)
    });
    let same_subnet = links.iter().any(|link| {
        link.kind != crate::LinkKind::Usb
            && match candidate.ip() {
                IpAddr::V4(address) => link.contains(address),
                IpAddr::V6(_) => false,
            }
    });
    // A candidate's link classification is only a routing preference. The
    // resume proof below remains the identity check, so a spoofed same-ID
    // advertisement cannot win merely by claiming USB.
    let rank = match classified {
        Some(crate::LinkKind::Usb) => 1,
        _ if same_subnet => 2,
        Some(crate::LinkKind::Wifi) => 3,
        Some(crate::LinkKind::BluetoothPan) => 4,
        Some(crate::LinkKind::Lan) => 5,
        None if candidate == original => 2,
        None => 6,
    };
    (rank, candidate)
}
