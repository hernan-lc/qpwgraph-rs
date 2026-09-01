//! Links qpwgraph owns on Windows, and the audio that makes them real.
//!
//! Core Audio reports which endpoint a session is attached to. It does not
//! offer a supported way to move one, which is why the observed session links
//! stay immutable. What Windows *does* allow from user mode is opening any
//! endpoint for capture, any playback endpoint for loopback, and any playback
//! endpoint for render — and once qpwgraph holds both ends it can carry the
//! audio between them itself.
//!
//! So a link drawn between two endpoint ports here is not a projection of
//! something Windows already decided. It is a route in [`crate::router`],
//! with real WASAPI streams at both ends, and disconnecting it stops real
//! audio. That is the whole difference between this module and the observed
//! graph next door.
//!
//! # What is routable
//!
//! | From | To | How |
//! | --- | --- | --- |
//! | a recording endpoint | a playback endpoint | capture → render |
//! | a playback endpoint's monitor | a playback endpoint | loopback → render |
//! | an application session | anything | refused |
//!
//! The last row is the honest one. Capturing a single application needs
//! process loopback, which needs a newer Windows build than this backend
//! targets, and re-pointing one needs an API Windows does not document. Both
//! are refused with a message rather than drawn as a link that carries
//! nothing.
//!
//! # Ownership
//!
//! The route table is always rebuilt from the link set and installed in one
//! transaction, so a rejected change leaves the working routes running. A
//! device is opened when the first link needs it and closed when the last one
//! stops, and the closing happens on the caller's thread rather than between
//! two blocks of audio.

use super::*;

use crate::router::engine::{
    DestinationSpec, RouteId, RouteSpec, RouterConfig, RouterError, SinkId, SourceId,
};
use crate::router::format::AudioFormat;
use crate::router::thread::{RouterStopped, RouterThread};
use crate::router::wasapi::{self, WasapiEndpoint};
use crate::router::RouteMetrics;

/// The format every qpwgraph-owned Windows route runs at.
///
/// WASAPI is asked to convert to it on both ends, so an endpoint at 44.1 kHz
/// or 7.1 still meets the router here and the router's own resampler is left
/// to handle drift rather than device geometry.
const ROUTE_FORMAT: AudioFormat = AudioFormat::new(48_000, 2);

/// Frames buffered between a device thread and the router, per stream.
///
/// 4096 frames is about 85 ms at 48 kHz: enough to absorb a scheduling
/// hiccup on either side, and bounded, so a stalled consumer loses audio and
/// increments a counter instead of growing latency without end.
const RING_FRAMES: usize = 4_096;

/// One WASAPI stream, kept alive for as long as some link needs it.
struct Device {
    endpoint: WasapiEndpoint,
    /// Links currently using this device. The stream closes when it reaches
    /// zero, so unplugging the last route also releases the hardware.
    users: usize,
}

/// The audio behind qpwgraph's own Windows links.
pub(super) struct WindowsRouting {
    router: RouterThread,
    /// Links this backend created, by id. The route table is derived from
    /// this map, never edited in place.
    links: BTreeMap<LinkId, Link>,
    sources: BTreeMap<PortId, (SourceId, Device)>,
    sinks: BTreeMap<PortId, (SinkId, Device)>,
    next_id: u64,
}

impl std::fmt::Debug for WindowsRouting {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowsRouting")
            .field("links", &self.links.len())
            .field("sources", &self.sources.len())
            .field("sinks", &self.sinks.len())
            .finish()
    }
}

impl WindowsRouting {
    pub(super) fn start() -> BackendResult<Self> {
        let router = RouterThread::start(RouterConfig {
            // 10 ms at 48 kHz: inside a WASAPI shared-mode period, and coarse
            // enough that per-block overhead stays negligible.
            block_frames: 480,
            clock_rate: ROUTE_FORMAT.sample_rate,
        })
        .map_err(|error| {
            BackendError::native(format!("could not start the Windows audio router: {error}"))
        })?;
        Ok(Self {
            router,
            links: BTreeMap::new(),
            sources: BTreeMap::new(),
            sinks: BTreeMap::new(),
            next_id: 1,
        })
    }

    pub(super) fn links(&self) -> impl Iterator<Item = &Link> {
        self.links.values()
    }

    pub(super) fn owns(&self, link: LinkId) -> bool {
        self.links.contains_key(&link)
    }

    /// What each route has actually been doing: frames carried, dropouts,
    /// conversion ratio, and the last fault.
    ///
    /// Reading these never touches the audio path — they are atomics — which
    /// is what makes "is this link carrying anything?" a question the UI can
    /// ask per frame.
    pub(super) fn metrics(&self) -> Vec<(LinkId, RouteMetrics)> {
        let mut out = Vec::with_capacity(self.links.len());
        for link in self.links.values() {
            // Routes are keyed by output port, so every link sharing a source
            // reports that route's counters.
            let route = RouteId(link.output_port.0);
            let Ok(Some(metrics)) = self.router.with(move |core| core.metrics(route)) else {
                continue;
            };
            out.push((link.id, metrics));
        }
        out
    }

    /// Carry audio from `output` to `input` for real.
    ///
    /// Both ports must be endpoint ports the router can open; a session port
    /// is refused here rather than accepted and quietly ignored. The devices
    /// are opened first, then the whole route table is reinstalled in one
    /// transaction, so a failure at either step leaves the previous routes
    /// exactly as they were.
    pub(super) fn connect(
        &mut self,
        link: Link,
        endpoint_ports: &BTreeMap<PortId, EndpointPort>,
    ) -> BackendResult<()> {
        if self.links.contains_key(&link.id) {
            return Err(BackendError::native("that route already exists"));
        }
        let output = routable(endpoint_ports, link.output_port, PortEnd::Output)?;
        let input = routable(endpoint_ports, link.input_port, PortEnd::Input)?;

        self.ensure_source(link.output_port, output)?;
        if let Err(error) = self.ensure_sink(link.input_port, input) {
            // Undo the half of the pair that did open, so a failed connect
            // leaves no device held open by nothing.
            self.release_source(link.output_port);
            return Err(error);
        }

        let (id, output_port, input_port) = (link.id, link.output_port, link.input_port);
        self.links.insert(id, link);
        match self.install() {
            Ok(()) => Ok(()),
            Err(error) => {
                self.links.remove(&id);
                self.release_source(output_port);
                self.release_sink(input_port);
                // The previous table is still installed, because `set_routes`
                // is all-or-nothing; there is nothing to restore.
                Err(error)
            }
        }
    }

    /// Stop carrying a route, releasing any device it was the last user of.
    pub(super) fn disconnect(&mut self, id: LinkId) -> BackendResult<Link> {
        let link = self
            .links
            .remove(&id)
            .ok_or_else(|| BackendError::native("that route is not one this backend created"))?;
        // Reinstalling first means the router has already let go of the
        // devices by the time they are closed.
        let result = self.install();
        self.release_source(link.output_port);
        self.release_sink(link.input_port);
        result?;
        Ok(link)
    }

    /// Drop every route whose ports no longer exist.
    ///
    /// Called after a graph rebuild: an unplugged device takes its ports with
    /// it, and a route pointing at a port that is gone is not a route. The
    /// links that survive are returned to the caller to re-add to the graph.
    pub(super) fn reconcile(&mut self, live_ports: &BTreeSet<PortId>) -> BackendResult<()> {
        let stale: Vec<Link> = self
            .links
            .values()
            .filter(|link| {
                !live_ports.contains(&link.output_port) || !live_ports.contains(&link.input_port)
            })
            .cloned()
            .collect();
        if stale.is_empty() {
            return Ok(());
        }
        for link in &stale {
            self.links.remove(&link.id);
        }
        let result = self.install();
        for link in &stale {
            self.release_source(link.output_port);
            self.release_sink(link.input_port);
        }
        result
    }

    /// Rebuild the route table from the current link set and install it.
    ///
    /// Fan-out is expressed here: every link sharing an output port becomes
    /// one route with several destinations, which is what lets the source be
    /// pulled exactly once per block however many places it is going.
    fn install(&mut self) -> BackendResult<()> {
        let mut by_source: BTreeMap<PortId, Vec<PortId>> = BTreeMap::new();
        for link in self.links.values() {
            by_source
                .entry(link.output_port)
                .or_default()
                .push(link.input_port);
        }

        let mut specs = Vec::with_capacity(by_source.len());
        for (output, inputs) in by_source {
            let Some((source, _)) = self.sources.get(&output) else {
                continue;
            };
            let destinations: Vec<DestinationSpec> = inputs
                .iter()
                .filter_map(|input| self.sinks.get(input))
                .map(|(sink, _)| DestinationSpec::new(*sink))
                .collect();
            if destinations.is_empty() {
                continue;
            }
            specs.push(RouteSpec {
                // Derived from the output port, so a route keeps its meters
                // and counters when a destination is added or removed.
                id: RouteId(output.0),
                source: *source,
                gain: 1.0,
                processors: Vec::new(),
                destinations,
            });
        }

        self.router
            .with(move |core| core.set_routes(&specs))
            .map_err(router_stopped)?
            .map_err(router_error)
    }

    /// Take a use of the device behind an output port, opening it if this is
    /// the first route that needs it.
    fn ensure_source(&mut self, port: PortId, endpoint: &EndpointPort) -> BackendResult<()> {
        if let Some((_, device)) = self.sources.get_mut(&port) {
            device.users += 1;
            return Ok(());
        }
        let device_id = Some(endpoint.device_id.as_str());
        let (source, wasapi) = match endpoint.role {
            EndpointPortRole::Capture => {
                wasapi::open_capture_source(device_id, ROUTE_FORMAT, RING_FRAMES)?
            }
            EndpointPortRole::Monitor => {
                wasapi::open_loopback_source(device_id, ROUTE_FORMAT, RING_FRAMES)?
            }
            EndpointPortRole::Render => {
                return Err(BackendError::unsupported(
                    "a playback device is not a source; drag from its monitor instead",
                ))
            }
        };
        let id = SourceId(self.take_id());
        self.router
            .with(move |core| core.add_source(id, Box::new(source)))
            .map_err(router_stopped)?
            .map_err(router_error)?;
        self.sources.insert(
            port,
            (
                id,
                Device {
                    endpoint: wasapi,
                    users: 1,
                },
            ),
        );
        Ok(())
    }

    fn ensure_sink(&mut self, port: PortId, endpoint: &EndpointPort) -> BackendResult<()> {
        if let Some((_, device)) = self.sinks.get_mut(&port) {
            device.users += 1;
            return Ok(());
        }
        if endpoint.role != EndpointPortRole::Render {
            return Err(BackendError::unsupported(
                "only a playback device can be the destination of a Windows audio route",
            ));
        }
        let (sink, wasapi) =
            wasapi::open_render_sink(Some(endpoint.device_id.as_str()), ROUTE_FORMAT, RING_FRAMES)?;
        let id = SinkId(self.take_id());
        self.router
            .with(move |core| core.add_sink(id, Box::new(sink)))
            .map_err(router_stopped)?
            .map_err(router_error)?;
        self.sinks.insert(
            port,
            (
                id,
                Device {
                    endpoint: wasapi,
                    users: 1,
                },
            ),
        );
        Ok(())
    }

    /// Give up one use of an output port's device, closing it if that was the
    /// last one.
    fn release_source(&mut self, port: PortId) {
        let Some((id, device)) = self.sources.get_mut(&port) else {
            return;
        };
        device.users -= 1;
        if device.users > 0 {
            return;
        }
        let id = *id;
        // The router hands the source back so it is dropped here rather than
        // between two blocks of audio, and the WASAPI thread is joined on
        // this thread for the same reason.
        let released = self.router.with(move |core| core.remove_source(id));
        drop(released);
        if let Some((_, mut device)) = self.sources.remove(&port) {
            device.endpoint.stop();
        }
    }

    fn release_sink(&mut self, port: PortId) {
        let Some((id, device)) = self.sinks.get_mut(&port) else {
            return;
        };
        device.users -= 1;
        if device.users > 0 {
            return;
        }
        let id = *id;
        let released = self.router.with(move |core| core.remove_sink(id));
        drop(released);
        if let Some((_, mut device)) = self.sinks.remove(&port) {
            device.endpoint.stop();
        }
    }

    fn take_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
}

enum PortEnd {
    Output,
    Input,
}

/// Resolve a port to the device behind it, or explain why it has none.
fn routable(
    endpoint_ports: &BTreeMap<PortId, EndpointPort>,
    port: PortId,
    end: PortEnd,
) -> BackendResult<&EndpointPort> {
    endpoint_ports.get(&port).ok_or_else(|| {
        // Almost always an application session. Say what Windows cannot do
        // rather than reporting a missing port the user can plainly see.
        BackendError::unsupported(match end {
            PortEnd::Output => {
                "only a recording device or a playback device's monitor can be the source of a \
                 Windows audio route; capturing one application needs process loopback, which \
                 this backend does not provide"
            }
            PortEnd::Input => {
                "only a playback device can be the destination of a Windows audio route; Windows \
                 exposes no supported way to re-point an application's stream"
            }
        })
    })
}

fn router_error(error: RouterError) -> BackendError {
    BackendError::native(format!("Windows audio route failed: {error}"))
}

fn router_stopped(_: RouterStopped) -> BackendError {
    BackendError::native("the Windows audio router is not running")
}

/// A stable link id for a pair of ports, so a route keeps its identity across
/// a graph rebuild.
pub(super) fn managed_link(output: PortId, input: PortId) -> Link {
    Link {
        id: LinkId(graph_id(managed_link_local_id(output, input))),
        output_port: output,
        input_port: input,
    }
}
