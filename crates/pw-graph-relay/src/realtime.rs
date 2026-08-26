//! Latency tuning for the audio path: socket quality-of-service and
//! best-effort realtime scheduling for the UDP worker threads.
//!
//! Everything here is advisory. A relay that cannot mark its packets or
//! cannot obtain a realtime policy still works; it simply competes for the
//! network and the CPU on ordinary terms. Failures are therefore swallowed
//! rather than surfaced — none of them is something the user can act on, and
//! an error toast for "no RLIMIT_RTPRIO" would be noise on every launch.

use socket2::SockRef;
use std::net::UdpSocket;

/// DSCP Expedited Forwarding (46) in the high six bits of the TOS byte.
/// Wi-Fi access points map EF onto the voice access category (AC_VO), which
/// is the single largest latency win available on a contended wireless link.
const DSCP_EF: u32 = 0xB8;

/// Send buffer, in bytes. Kept small on purpose: a deep socket queue only
/// converts a transient stall into standing delay, and the frame in hand is
/// always more useful than the one behind it.
const SEND_BUFFER_BYTES: usize = 64 * 1024;

/// Receive buffer, in bytes. Larger than the send side because datagrams
/// that arrive during a scheduling hiccup are worth keeping — the jitter
/// buffer reorders them by sequence and discards whatever turns out stale.
const RECV_BUFFER_BYTES: usize = 256 * 1024;

/// Apply audio quality-of-service to a relay UDP socket.
pub(crate) fn tune_audio_socket(socket: &UdpSocket) {
    let socket = SockRef::from(socket);
    #[cfg(unix)]
    match socket.local_addr().map(|addr| addr.is_ipv4()) {
        Ok(true) | Err(_) => {
            let _ = socket.set_tos(DSCP_EF);
        }
        Ok(false) => {
            let _ = socket.set_tclass_v6(DSCP_EF);
        }
    }
    #[cfg(not(unix))]
    {
        // socket2 exposes IPv4 TOS on Windows, while the Unix-only IPv6
        // tclass helper is not available on that target. The IPv4 setting is
        // still useful for the normal relay path and remains best effort.
        let _ = socket.set_tos(DSCP_EF);
    }
    let _ = socket.set_send_buffer_size(SEND_BUFFER_BYTES);
    let _ = socket.set_recv_buffer_size(RECV_BUFFER_BYTES);
}

/// Ask the kernel to schedule the calling thread as realtime.
///
/// The relay's send and receive loops sit between two audio deadlines, so an
/// ordinary time-sharing slice can delay a frame by more than the frame
/// itself lasts. `SCHED_FIFO` at a low priority avoids that without starving
/// anything: the threads block on a socket or a condvar almost all the time.
///
/// This normally fails for an unprivileged process without `RLIMIT_RTPRIO`,
/// which is expected and harmless.
#[cfg(target_os = "linux")]
pub(crate) fn request_realtime_thread() {
    // Well below PipeWire's own realtime threads (which run in the high
    // tens): the relay must never preempt the graph that feeds it.
    const RELAY_RT_PRIORITY: libc::c_int = 5;

    // SAFETY: `param` is a correctly initialized `sched_param`, and pid 0
    // names the calling thread. The call only reads the struct.
    unsafe {
        let param = libc::sched_param {
            sched_priority: RELAY_RT_PRIORITY,
        };
        libc::sched_setscheduler(0, libc::SCHED_FIFO, &param);
    }
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn request_realtime_thread() {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn tuning_a_socket_never_fails_the_caller() {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
        tune_audio_socket(&socket);
        // The socket must still be usable afterwards, whatever the kernel
        // made of the individual options.
        let addr = socket.local_addr().expect("local addr");
        socket.send_to(b"probe", addr).expect("send");
    }

    #[test]
    fn requesting_realtime_is_safe_without_privileges() {
        request_realtime_thread();
    }
}
