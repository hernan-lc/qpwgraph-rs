//! Fast-mode link classification and selection.
//!
//! Relay traffic only runs over fast local links: USB tethering, Wi-Fi,
//! Bluetooth PAN, and (for Linux-to-Linux peers) wired LAN. Interfaces are
//! classified by name (Linux predictable-naming heuristics), ranked by a
//! fixed latency/stability policy — USB > Wi-Fi > Bluetooth PAN > LAN — and
//! a same-subnet match against the peer wins over raw ranking so traffic
//! never leaves the local segment unnecessarily.

use netdev::get_interfaces;
use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

// A kind of local network link, declared in preference order: USB
// tethering first (lowest latency, most stable), wired LAN last.
pw_graph_utils::enum_str! {
    #[derive(PartialOrd, Ord, Hash)]
    pub enum LinkKind {
        Usb = "usb",
        Wifi = "wifi",
        BluetoothPan = "bluetooth",
        Lan = "lan",
    }
}

impl LinkKind {
    /// Human-readable label for UI display.
    pub fn label(self) -> &'static str {
        match self {
            Self::Usb => "USB",
            Self::Wifi => "Wi-Fi",
            Self::BluetoothPan => "Bluetooth PAN",
            Self::Lan => "LAN",
        }
    }
}

/// Classify a network interface by name. Returns `None` for interfaces the
/// relay never uses (loopback, virtual bridges, tunnels, unknown names).
pub fn classify_interface(name: &str) -> Option<LinkKind> {
    let name = name.to_ascii_lowercase();

    // Wi-Fi first: nothing else starts with `wl`.
    if name.starts_with("wlan") || name.starts_with("wlp") || name.starts_with("wifi") {
        return Some(LinkKind::Wifi);
    }
    // USB tethering and USB Ethernet dongles.
    if name.starts_with("usb")
        || name.starts_with("rndis")
        || name.starts_with("ncm")
        || name.starts_with("enx")
        || is_usb_predictable_ethernet_name(&name)
    {
        return Some(LinkKind::Usb);
    }
    // Bluetooth PAN (bnep devices and common bridge names).
    if name.starts_with("bnep") || name.starts_with("bt-pan") || name.starts_with("pan") {
        return Some(LinkKind::BluetoothPan);
    }
    // Wired Ethernet: accepted for Linux-to-Linux peers. The `en*` USB rule
    // above already took `enp0s20u1`-style names, so this is plain LAN.
    if name.starts_with("eth")
        || name.starts_with("enp")
        || name.starts_with("eno")
        || name.starts_with("ens")
        || name.starts_with("em")
    {
        return Some(LinkKind::Lan);
    }
    None
}

/// Linux predictable names for an Ethernet interface attached below a USB
/// path commonly contain a `u` component (for example `enp0s20u1`). Keep the
/// heuristic narrow: arbitrary `en*` names and WWAN/mobile-broadband names
/// must remain LAN/unknown rather than being preferred as USB tether links.
fn is_usb_predictable_ethernet_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("enp") else {
        return false;
    };
    rest.split('u')
        .nth(1)
        .and_then(|suffix| suffix.chars().next())
        .is_some_and(|first| first.is_ascii_digit())
}

// Which transport the user asked for. `Auto` (the default) picks the best
// available link by policy.
pw_graph_utils::enum_str! {
    #[derive(Default)]
    pub enum TransportPreference {
        #[default]
        Auto = "auto",
        Usb = "usb",
        Wifi = "wifi",
        Bluetooth = "bluetooth",
        Lan = "lan",
    }
}

impl TransportPreference {
    fn matches(self, kind: LinkKind) -> bool {
        match self {
            Self::Auto => true,
            Self::Usb => kind == LinkKind::Usb,
            Self::Wifi => kind == LinkKind::Wifi,
            Self::Bluetooth => kind == LinkKind::BluetoothPan,
            Self::Lan => kind == LinkKind::Lan,
        }
    }
}

impl FromStr for TransportPreference {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text.trim().to_ascii_lowercase().as_str() {
            "" | "auto" => Ok(Self::Auto),
            "usb" => Ok(Self::Usb),
            "wifi" | "wlan" => Ok(Self::Wifi),
            "bluetooth" | "bt" => Ok(Self::Bluetooth),
            "lan" | "ethernet" => Ok(Self::Lan),
            other => Err(format!("unknown transport preference '{other}'")),
        }
    }
}

/// One usable local IPv4 link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalLink {
    pub name: String,
    pub addr: Ipv4Addr,
    pub netmask: Ipv4Addr,
    pub kind: LinkKind,
}

impl LocalLink {
    /// Whether `target` sits on this link's subnet.
    pub fn contains(&self, target: Ipv4Addr) -> bool {
        let mask = u32::from(self.netmask);
        u32::from(self.addr) & mask == u32::from(target) & mask
    }

    /// The best active USB tethering link, if one is up. Links come back
    /// ranked USB-first, so the first USB entry is the preferred one.
    pub fn find_usb() -> Option<LocalLink> {
        local_links()
            .into_iter()
            .find(|link| link.kind == LinkKind::Usb)
    }
}

/// Enumerate usable local IPv4 links, sorted best-first (policy rank, then
/// address for determinism). Requires no privileges on Linux.
pub fn local_links() -> Vec<LocalLink> {
    enumerate_links()
}

/// Enumerate addresses that can be shown to a user as relay endpoints.
///
/// Transport selection stays deliberately strict in [`local_links`]: it only
/// returns links whose carrier and interface flags prove that the relay can
/// use them.  Some desktop environments (and sandboxed/network-managed
/// setups) do not expose those flags consistently even though the interface
/// has a valid address and is the default route.  In that case the UI would
/// otherwise hide both the endpoint and the QR code while the host is already
/// listening successfully.
///
/// Use the strict list whenever possible.  If it is empty, fall back to
/// non-loopback IPv4 addresses on the default or physical interfaces.  The
/// fallback is for displaying the address and building the pairing QR only;
/// relay workers continue to use [`local_links`] for interface binding.
pub fn display_links() -> Vec<LocalLink> {
    let links = local_links();
    if !links.is_empty() {
        return links;
    }

    let interfaces = get_interfaces();
    let preferred = collect_display_links(&interfaces, true);
    if !preferred.is_empty() {
        return preferred;
    }

    // Last resort for hosts where netdev cannot report default/physical
    // metadata (common inside containers and some desktop sandboxes). The
    // address is still more useful than hiding the port and QR altogether.
    collect_display_links(&interfaces, false)
}

/// Append every usable IPv4 address of `interface` to `links`. Link-local
/// and unspecified addresses are skipped because they are not routable to
/// a peer. Both the strict and display paths share this filter.
fn push_v4_links(links: &mut Vec<LocalLink>, interface: &netdev::Interface, kind: LinkKind) {
    for v4 in &interface.ipv4 {
        let addr = v4.addr();
        if addr.is_link_local() || addr.is_unspecified() {
            continue;
        }
        links.push(LocalLink {
            name: interface.name.clone(),
            addr,
            netmask: v4.netmask(),
            kind,
        });
    }
}

/// Sort links best-first: policy rank, then address for deterministic order.
/// Every public link list is ranked this way so "first entry wins" heuristics
/// (USB tethering, bind address selection) behave consistently.
fn sort_links(links: &mut [LocalLink]) {
    links.sort_by(|a, b| {
        (a.kind, a.addr)
            .partial_cmp(&(b.kind, b.addr))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

fn collect_display_links(interfaces: &[netdev::Interface], preferred_only: bool) -> Vec<LocalLink> {
    let mut links = Vec::new();
    for interface in interfaces {
        if interface.is_loopback()
            || (preferred_only && !interface.default && !interface.is_physical())
        {
            continue;
        }
        let kind = classify_interface(&interface.name).unwrap_or(LinkKind::Lan);
        push_v4_links(&mut links, interface, kind);
    }
    sort_links(&mut links);
    links
}

/// Enumerate local IPv4 links that a peer can actually reach: only interfaces
/// that are up, running (carrier present), and named like a real fast link are
/// reported. Down interfaces, loopback, virtual bridges, and unclassified
/// interfaces are skipped entirely, so the QR and endpoint list never show an
/// address that is not usable for relay traffic.
fn enumerate_links() -> Vec<LocalLink> {
    let mut links = Vec::new();
    for interface in get_interfaces() {
        // A configured address on a down interface, or on an interface with no
        // carrier, is not a usable link. `is_running` on Linux tracks the
        // carrier state, so a disconnected ethernet or an unassociated Wi-Fi
        // interface stays hidden even though it still has an address.
        if interface.is_loopback() || !interface.is_up() || !interface.is_running() {
            continue;
        }
        let Some(kind) = classify_interface(&interface.name) else {
            // Not a classified fast link (tunnel, virtual bridge, unknown
            // name): never fall back to a generic LAN label.
            continue;
        };
        push_v4_links(&mut links, &interface, kind);
    }
    sort_links(&mut links);
    links
}

/// Filter and rank links for a preference. `Auto` keeps everything ranked
/// by policy; a specific preference may yield an empty list when that link
/// kind is currently absent. Input order does not matter.
pub fn select_links(links: &[LocalLink], preference: TransportPreference) -> Vec<LocalLink> {
    let mut selected: Vec<LocalLink> = links
        .iter()
        .filter(|link| preference.matches(link.kind))
        .cloned()
        .collect();
    sort_links(&mut selected);
    selected
}

/// Choose the local address to bind when reaching `target`: a same-subnet
/// link wins; otherwise the highest-ranked candidate. Returns `None` when
/// no candidate links exist (the OS default route is used then).
pub fn outbound_bind_addr(
    links: &[LocalLink],
    target: SocketAddr,
    preference: TransportPreference,
) -> Option<Ipv4Addr> {
    let IpAddr::V4(target_v4) = target.ip() else {
        return None;
    };
    let candidates = select_links(links, preference);
    candidates
        .iter()
        .find(|link| link.contains(target_v4))
        .or_else(|| candidates.first())
        .map(|link| link.addr)
}

/// Choose the local address a host should listen on for a transport
/// preference. `Auto` selects the highest-ranked active relay link. If an
/// explicit preference has no matching link, the best active link is used as
/// a safe fallback. `None` means that no usable link information exists; the
/// caller may then use its documented all-interface fallback.
///
/// Selecting "USB tether" used to change only which link outbound
/// connections preferred, while the listener still accepted pairing on the
/// LAN, on every VPN, and on anything else that happened to be up. Honouring
/// the preference here makes the choice mean what it says.
pub fn listen_bind_addr(links: &[LocalLink], preference: TransportPreference) -> Option<Ipv4Addr> {
    if let Some(link) = select_links(links, preference).first() {
        return Some(link.addr);
    }
    select_links(links, TransportPreference::Auto)
        .first()
        .map(|link| link.addr)
}

/// Connect a TCP stream, optionally bound to a specific local address.
///
/// When no bind address is requested the standard
/// [`TcpStream::connect_timeout`](std::net::TcpStream::connect_timeout) path
/// is used — it is simpler and works for loopback tests. Binding to a local
/// address needs socket2 so the interface selection policy can stick.
pub fn connect_tcp(
    target: SocketAddr,
    bind: Option<Ipv4Addr>,
    timeout: Duration,
) -> std::io::Result<std::net::TcpStream> {
    connect_tcp_inner(target, bind, timeout, None)
}

/// Cancellable variant used by bounded discovery probes. A connect that is
/// already in progress is interrupted as soon as the scanner is stopped;
/// callers that do not need cancellation should use [`connect_tcp`].
pub fn connect_tcp_cancellable(
    target: SocketAddr,
    bind: Option<Ipv4Addr>,
    timeout: Duration,
    cancel: &AtomicBool,
) -> std::io::Result<std::net::TcpStream> {
    connect_tcp_inner(target, bind, timeout, Some(cancel))
}

fn connect_tcp_inner(
    target: SocketAddr,
    bind: Option<Ipv4Addr>,
    timeout: Duration,
    cancel: Option<&AtomicBool>,
) -> std::io::Result<std::net::TcpStream> {
    if cancel.is_some_and(|cancel| cancel.load(Ordering::Acquire)) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "TCP connect cancelled",
        ));
    }
    let Some(local) = bind else {
        let stream = std::net::TcpStream::connect_timeout(&target, timeout)?;
        let _ = stream.set_nodelay(true);
        return Ok(stream);
    };

    let domain = if target.is_ipv4() {
        Domain::IPV4
    } else {
        Domain::IPV6
    };
    let socket = Socket::new(domain, Type::STREAM, Some(Protocol::TCP))?;
    socket.bind(&SockAddr::from(SocketAddr::new(IpAddr::V4(local), 0)))?;
    socket.set_nonblocking(true)?;
    match socket.connect(&SockAddr::from(target)) {
        Ok(()) => {}
        // Non-blocking connect is in progress (EINPROGRESS on Linux, often
        // WouldBlock elsewhere). Poll until connected or the deadline hits.
        Err(error) if is_connect_in_progress(&error) => {
            wait_for_connect(&socket, timeout, cancel)?;
        }
        Err(error) => return Err(error),
    }
    socket.set_nonblocking(false)?;
    socket.set_nodelay(true)?;
    Ok(socket.into())
}

fn is_connect_in_progress(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
    ) || error.raw_os_error() == Some(115) // EINPROGRESS on Linux
}

fn wait_for_connect(
    socket: &Socket,
    timeout: Duration,
    cancel: Option<&AtomicBool>,
) -> std::io::Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        if cancel.is_some_and(|cancel| cancel.load(Ordering::Acquire)) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "TCP connect cancelled",
            ));
        }
        if let Some(error) = socket.take_error()? {
            return Err(error);
        }
        if socket.peer_addr().is_ok() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "TCP connect timed out",
            ));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Measure TCP connect latency to `target`, optionally from a specific local
/// address. Used to break ties between several candidate links.
pub fn probe_tcp_rtt(
    target: SocketAddr,
    bind: Option<Ipv4Addr>,
    timeout: Duration,
) -> Option<Duration> {
    let start = Instant::now();
    connect_tcp(target, bind, timeout).ok()?;
    Some(start.elapsed())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_common_interface_names() {
        let cases = [
            ("wlan0", LinkKind::Wifi),
            ("wlp3s0", LinkKind::Wifi),
            ("wifi0", LinkKind::Wifi),
            ("usb0", LinkKind::Usb),
            ("rndis0", LinkKind::Usb),
            ("enx00e04c680000", LinkKind::Usb),
            ("enp0s20u1", LinkKind::Usb),
            ("ncm0", LinkKind::Usb),
            ("bnep0", LinkKind::BluetoothPan),
            ("bt-pan0", LinkKind::BluetoothPan),
            ("pan0", LinkKind::BluetoothPan),
            ("eth0", LinkKind::Lan),
            ("enp5s0", LinkKind::Lan),
            ("eno1", LinkKind::Lan),
            ("ens33", LinkKind::Lan),
            ("em1", LinkKind::Lan),
        ];
        for (name, expected) in cases {
            assert_eq!(classify_interface(name), Some(expected), "interface {name}");
        }
    }

    #[test]
    fn ignores_unusable_interfaces() {
        for name in [
            "lo", "veth0", "docker0", "virbr0", "tun0", "br0", "mystery0", "wwan0",
        ] {
            assert_eq!(classify_interface(name), None, "interface {name}");
        }
    }

    #[test]
    fn classification_is_case_insensitive() {
        assert_eq!(classify_interface("WLAN0"), Some(LinkKind::Wifi));
        assert_eq!(classify_interface("USB0"), Some(LinkKind::Usb));
    }

    #[test]
    fn policy_orders_usb_first_and_lan_last() {
        assert!(LinkKind::Usb < LinkKind::Wifi);
        assert!(LinkKind::Wifi < LinkKind::BluetoothPan);
        assert!(LinkKind::BluetoothPan < LinkKind::Lan);
    }

    #[test]
    fn preference_parsing_round_trips() {
        for preference in [
            TransportPreference::Auto,
            TransportPreference::Usb,
            TransportPreference::Wifi,
            TransportPreference::Bluetooth,
            TransportPreference::Lan,
        ] {
            let parsed: TransportPreference = preference.as_str().parse().unwrap();
            assert_eq!(parsed, preference);
        }
        assert_eq!("bt".parse(), Ok(TransportPreference::Bluetooth));
        assert_eq!("ethernet".parse(), Ok(TransportPreference::Lan));
        assert_eq!("".parse(), Ok(TransportPreference::Auto));
        assert!("carrier-pigeon".parse::<TransportPreference>().is_err());
    }

    fn fixture_links() -> Vec<LocalLink> {
        let mk = |name: &str, addr: [u8; 4], kind: LinkKind| LocalLink {
            name: name.into(),
            addr: Ipv4Addr::from(addr),
            netmask: Ipv4Addr::new(255, 255, 255, 0),
            kind,
        };
        vec![
            mk("enp5s0", [192, 168, 1, 10], LinkKind::Lan),
            mk("usb0", [192, 168, 42, 129], LinkKind::Usb),
            mk("wlan0", [192, 168, 1, 11], LinkKind::Wifi),
            mk("bnep0", [10, 7, 0, 1], LinkKind::BluetoothPan),
        ]
    }

    #[test]
    fn select_links_ranks_by_policy() {
        let mut links = fixture_links();
        links.sort_by_key(|a| a.kind);
        let ranked = select_links(&links, TransportPreference::Auto);
        let kinds: Vec<LinkKind> = ranked.iter().map(|link| link.kind).collect();
        assert_eq!(
            kinds,
            vec![
                LinkKind::Usb,
                LinkKind::Wifi,
                LinkKind::BluetoothPan,
                LinkKind::Lan
            ]
        );
        let wifi_only = select_links(&links, TransportPreference::Wifi);
        assert_eq!(wifi_only.len(), 1);
        assert_eq!(wifi_only[0].kind, LinkKind::Wifi);
    }

    #[test]
    fn a_transport_preference_constrains_the_listener() {
        let links = fixture_links();
        assert_eq!(
            listen_bind_addr(&links, TransportPreference::Auto),
            Some(Ipv4Addr::new(192, 168, 42, 129))
        );
        let usb = listen_bind_addr(&links, TransportPreference::Usb);
        assert!(usb.is_some());
        assert_eq!(
            usb,
            select_links(&links, TransportPreference::Usb)
                .first()
                .map(|link| link.addr)
        );
    }

    #[test]
    fn listener_selection_uses_the_best_available_link_and_safe_fallbacks() {
        let mk = |kind: LinkKind, addr: Ipv4Addr| LocalLink {
            name: kind.as_str().into(),
            addr,
            netmask: Ipv4Addr::new(255, 255, 255, 0),
            kind,
        };
        let wifi = mk(LinkKind::Wifi, Ipv4Addr::new(192, 168, 1, 2));
        let bluetooth = mk(LinkKind::BluetoothPan, Ipv4Addr::new(10, 0, 0, 2));
        let lan = mk(LinkKind::Lan, Ipv4Addr::new(172, 16, 0, 2));
        assert_eq!(
            listen_bind_addr(
                &[
                    wifi.clone(),
                    mk(LinkKind::Usb, Ipv4Addr::new(192, 168, 42, 2))
                ],
                TransportPreference::Auto
            ),
            Some(Ipv4Addr::new(192, 168, 42, 2))
        );
        assert_eq!(
            listen_bind_addr(&[wifi.clone()], TransportPreference::Auto),
            Some(wifi.addr)
        );
        assert_eq!(
            listen_bind_addr(&[lan.clone(), bluetooth.clone()], TransportPreference::Auto),
            Some(bluetooth.addr)
        );
        assert_eq!(
            listen_bind_addr(&[lan.clone(), wifi.clone()], TransportPreference::Wifi),
            Some(wifi.addr)
        );
        assert_eq!(
            listen_bind_addr(&[lan, wifi.clone()], TransportPreference::Usb),
            Some(wifi.addr)
        );
        assert_eq!(listen_bind_addr(&[], TransportPreference::Auto), None);
    }

    #[test]
    fn bind_addr_prefers_same_subnet_over_rank() {
        let links = fixture_links();
        // Same subnet as wlan0/enp5s0: Wi-Fi wins over the (higher-ranked)
        // USB link because traffic must not leave the local segment.
        let target = SocketAddr::new(Ipv4Addr::new(192, 168, 1, 50).into(), 48123);
        let bind = outbound_bind_addr(&links, target, TransportPreference::Auto);
        let chosen = bind.expect("a link matches the target subnet");
        assert!(
            chosen == Ipv4Addr::new(192, 168, 1, 10) || chosen == Ipv4Addr::new(192, 168, 1, 11),
            "expected a 192.168.1.x link, got {chosen}"
        );
        // Off-subnet target falls back to the policy leader (USB).
        let remote = SocketAddr::new(Ipv4Addr::new(172, 31, 0, 5).into(), 48123);
        assert_eq!(
            outbound_bind_addr(&links, remote, TransportPreference::Auto),
            Some(Ipv4Addr::new(192, 168, 42, 129))
        );
        // An explicit preference is honoured even off-subnet.
        assert_eq!(
            outbound_bind_addr(&links, remote, TransportPreference::Lan),
            Some(Ipv4Addr::new(192, 168, 1, 10))
        );
        // No matching links at all.
        assert_eq!(
            outbound_bind_addr(&[], remote, TransportPreference::Auto),
            None
        );
    }

    #[test]
    fn subnet_membership_is_mask_based() {
        let link = LocalLink {
            name: "wlan0".into(),
            addr: Ipv4Addr::new(192, 168, 1, 11),
            netmask: Ipv4Addr::new(255, 255, 255, 0),
            kind: LinkKind::Wifi,
        };
        assert!(link.contains(Ipv4Addr::new(192, 168, 1, 200)));
        assert!(!link.contains(Ipv4Addr::new(192, 168, 2, 1)));
    }
}
