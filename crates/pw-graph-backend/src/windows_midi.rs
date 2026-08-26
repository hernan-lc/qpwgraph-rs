//! Windows MIDI backend built on WinMM.
//!
//! Unlike Core Audio, MIDI on Windows has genuine routing: `midiConnect` wires
//! an input device straight to an output device inside the MIDI stack, and
//! `midiDisconnect` takes it apart again. This is therefore the first Windows
//! backend where `connect` is honestly `true` -- the graph is editable, not
//! observed.
//!
//! It is not equivalent to ALSA MIDI. Windows connects one input to one output
//! at a time unless a MIDI-thru driver is involved, so fan-out from a single
//! input is refused rather than silently dropping the previous connection.
//! Fan-in works: several inputs may drive the same output.
//!
//! Handles are opened lazily and only for devices that take part in a
//! connection, so merely listing the graph never opens a device another
//! application might want.

use crate::api::{BackendCapabilities, BackendError, BackendResult, EffectDriver, GraphDriver};
use pw_graph_core::{
    encode_backend_id, BackendNamespace, Direction, Graph, GraphError, Link, LinkId, Node, NodeId,
    Port, PortId, PortType,
};
use std::collections::BTreeMap;

use windows::Win32::Media::Audio;
use windows::Win32::Media::Multimedia::{DRV_QUERYDEVICEINTERFACE, DRV_QUERYDEVICEINTERFACESIZE};

const WINDOWS_MIDI_CAPABILITIES: BackendCapabilities = BackendCapabilities {
    topology: true,
    // The one Windows backend that can really rewire itself.
    connect: true,
    disconnect: true,
    volume: false,
    mute: false,
    meters: false,
    effects: false,
    relay: false,
};

/// `MMSYSERR_NOERROR`.
const MM_OK: u32 = 0;

/// Local id space: inputs and outputs are numbered separately by WinMM, so the
/// kind is folded into the id to keep them distinct inside one namespace. The
/// lower 52 bits hold a hash of the opaque Plug and Play interface name; the
/// upper four local bits identify the graph resource kind.
const INPUT_TAG: u64 = 0x0010_0000_0000_0000;
const OUTPUT_TAG: u64 = 0x0020_0000_0000_0000;
const PORT_TAG: u64 = 0x0040_0000_0000_0000;
const IDENTITY_HASH_MASK: u64 = 0x000F_FFFF_FFFF_FFFF;

fn graph_id(local: u64) -> u64 {
    encode_backend_id(BackendNamespace::WindowsMidi, local)
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum DeviceKind {
    Input,
    Output,
}

#[derive(Clone, Debug)]
struct MidiDevice {
    /// WinMM device index, which is what every `midi*` call takes.
    index: u32,
    kind: DeviceKind,
    name: String,
    node_id: NodeId,
    port_id: PortId,
}

/// An open pair kept alive for as long as the connection exists. Closing
/// either handle tears the connection down, so both are owned here.
struct OpenConnection {
    input: Audio::HMIDIIN,
    output: Audio::HMIDIOUT,
    link_id: LinkId,
}

/// Closing either handle already tears the connection down, but disconnecting
/// first keeps the MIDI stack's own bookkeeping tidy. Doing this in `Drop`
/// means a connection cannot be forgotten anywhere it is discarded.
impl Drop for OpenConnection {
    fn drop(&mut self) {
        unsafe {
            let _ = Audio::midiDisconnect(Audio::HMIDI(self.input.0), self.output, None);
            let _ = Audio::midiOutClose(self.output);
            let _ = Audio::midiInClose(self.input);
        }
    }
}

/// WinMM MIDI graph. Handles live here and are closed on drop.
#[derive(Default)]
pub struct WindowsMidiDriver {
    graph: Graph,
    devices: BTreeMap<PortId, MidiDevice>,
    connections: Vec<OpenConnection>,
    positions: BTreeMap<NodeId, [f32; 2]>,
    next_link: u64,
}

impl std::fmt::Debug for WindowsMidiDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowsMidiDriver")
            .field("devices", &self.devices.len())
            .field("connections", &self.connections.len())
            .finish()
    }
}

impl WindowsMidiDriver {
    pub fn new() -> BackendResult<Self> {
        let mut driver = Self {
            next_link: 1,
            ..Self::default()
        };
        driver.refresh()?;
        Ok(driver)
    }

    /// Devices WinMM currently reports, in index order per direction. The
    /// device index is retained only for calls into WinMM; graph identity is
    /// based on the device interface name whenever the system provides one.
    fn enumerate() -> Vec<MidiDevice> {
        let mut devices = Vec::new();
        let mut identities = BTreeMap::<(DeviceKind, String), u32>::new();
        for index in 0..unsafe { Audio::midiInGetNumDevs() } {
            let mut caps = Audio::MIDIINCAPSW::default();
            let status = unsafe {
                Audio::midiInGetDevCapsW(
                    index as usize,
                    &mut caps,
                    std::mem::size_of::<Audio::MIDIINCAPSW>() as u32,
                )
            };
            if status != MM_OK {
                continue;
            }
            // The caps structs are packed, so the name is copied out before it
            // can be referenced.
            let name = { caps.szPname };
            let name = wide_name(&name);
            let manufacturer = { caps.wMid };
            let product = { caps.wPid };
            let driver_version = { caps.vDriverVersion };
            let base_identity =
                query_device_interface(DeviceKind::Input, index).unwrap_or_else(|| {
                    fallback_identity(
                        DeviceKind::Input,
                        &name,
                        manufacturer,
                        product,
                        driver_version,
                    )
                });
            let identity = unique_identity(DeviceKind::Input, base_identity, &mut identities);
            devices.push(device_from_identity(
                index,
                DeviceKind::Input,
                name,
                &identity,
            ));
        }
        for index in 0..unsafe { Audio::midiOutGetNumDevs() } {
            let mut caps = Audio::MIDIOUTCAPSW::default();
            let status = unsafe {
                Audio::midiOutGetDevCapsW(
                    index as usize,
                    &mut caps,
                    std::mem::size_of::<Audio::MIDIOUTCAPSW>() as u32,
                )
            };
            if status != MM_OK {
                continue;
            }
            let name = { caps.szPname };
            let name = wide_name(&name);
            let manufacturer = { caps.wMid };
            let product = { caps.wPid };
            let driver_version = { caps.vDriverVersion };
            let base_identity =
                query_device_interface(DeviceKind::Output, index).unwrap_or_else(|| {
                    fallback_identity(
                        DeviceKind::Output,
                        &name,
                        manufacturer,
                        product,
                        driver_version,
                    )
                });
            let identity = unique_identity(DeviceKind::Output, base_identity, &mut identities);
            devices.push(device_from_identity(
                index,
                DeviceKind::Output,
                name,
                &identity,
            ));
        }
        devices
    }

    fn device(&self, port: PortId) -> Option<&MidiDevice> {
        self.devices.get(&port)
    }

    /// Resolve a connect request into the input and output it names.
    fn endpoints(&self, src: PortId, dst: PortId) -> BackendResult<(MidiDevice, MidiDevice)> {
        let source = self
            .device(src)
            .cloned()
            .ok_or(GraphError::MissingPort(src))?;
        let destination = self
            .device(dst)
            .cloned()
            .ok_or(GraphError::MissingPort(dst))?;
        if source.kind != DeviceKind::Input || destination.kind != DeviceKind::Output {
            return Err(BackendError::unsupported(
                "a MIDI connection runs from an input device to an output device",
            ));
        }
        Ok((source, destination))
    }

    fn allocate_link(&mut self) -> LinkId {
        let id = LinkId(graph_id(self.next_link));
        self.next_link += 1;
        id
    }
}

#[cfg(test)]
fn device_from_caps(index: u32, kind: DeviceKind, name: String) -> MidiDevice {
    let identity = fallback_identity(kind, &name, 0, 0, 0);
    device_from_identity(index, kind, name, &identity)
}

fn device_from_identity(index: u32, kind: DeviceKind, name: String, identity: &str) -> MidiDevice {
    let tag = match kind {
        DeviceKind::Input => INPUT_TAG,
        DeviceKind::Output => OUTPUT_TAG,
    };
    let hash = stable_identity_hash(kind, identity);
    let local = tag | hash;
    MidiDevice {
        index,
        kind,
        name,
        node_id: NodeId(graph_id(local)),
        port_id: PortId(graph_id(PORT_TAG | hash)),
    }
}

fn fallback_identity(
    kind: DeviceKind,
    name: &str,
    manufacturer: u16,
    product: u16,
    driver_version: u32,
) -> String {
    format!(
        "fallback:{}:{manufacturer:04x}:{product:04x}:{driver_version:08x}:{}",
        kind.as_str(),
        name.trim()
    )
}

fn unique_identity(
    kind: DeviceKind,
    base: String,
    identities: &mut BTreeMap<(DeviceKind, String), u32>,
) -> String {
    let occurrence = identities.entry((kind, base.clone())).or_insert(0);
    let identity = if *occurrence == 0 {
        base
    } else {
        format!("{}#{}", base, *occurrence + 1)
    };
    *occurrence += 1;
    identity
}

impl DeviceKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Output => "output",
        }
    }
}

fn stable_identity_hash(kind: DeviceKind, identity: &str) -> u64 {
    // FNV-1a is small, deterministic, and sufficient after the opaque system
    // identity has already made the device distinction. The kind is included
    // again so a fallback string can never alias across directions.
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in format!("winmm-midi:{}:{identity}", kind.as_str()).bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    (hash & IDENTITY_HASH_MASK).max(1)
}

/// Query the stable Plug and Play interface name without opening the MIDI
/// device. WinMM requires the numeric device id to be cast to its handle type
/// for these two system-intercepted messages; the returned string is opaque
/// and is only used as a stable identity key.
fn query_device_interface(kind: DeviceKind, index: u32) -> Option<String> {
    let mut size = 0u32;
    let device = index as usize as *mut core::ffi::c_void;
    let status = unsafe {
        match kind {
            DeviceKind::Input => Audio::midiInMessage(
                Some(Audio::HMIDIIN(device)),
                DRV_QUERYDEVICEINTERFACESIZE,
                Some((&mut size as *mut u32) as usize),
                Some(0),
            ),
            DeviceKind::Output => Audio::midiOutMessage(
                Some(Audio::HMIDIOUT(device)),
                DRV_QUERYDEVICEINTERFACESIZE,
                Some((&mut size as *mut u32) as usize),
                Some(0),
            ),
        }
    };
    if status != MM_OK || size == 0 || size > 1024 * 1024 {
        return None;
    }
    let mut buffer = vec![0u16; (size as usize).div_ceil(std::mem::size_of::<u16>())];
    let status = unsafe {
        match kind {
            DeviceKind::Input => Audio::midiInMessage(
                Some(Audio::HMIDIIN(device)),
                DRV_QUERYDEVICEINTERFACE,
                Some(buffer.as_mut_ptr() as usize),
                Some(size as usize),
            ),
            DeviceKind::Output => Audio::midiOutMessage(
                Some(Audio::HMIDIOUT(device)),
                DRV_QUERYDEVICEINTERFACE,
                Some(buffer.as_mut_ptr() as usize),
                Some(size as usize),
            ),
        }
    };
    (status == MM_OK)
        .then(|| wide_name(&buffer))
        .filter(|name| !name.is_empty())
}

/// WinMM device names are a fixed-size, NUL-padded UTF-16 array.
fn wide_name(raw: &[u16]) -> String {
    let end = raw
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(raw.len());
    String::from_utf16_lossy(&raw[..end])
}

fn mm_error(operation: &str, status: u32) -> BackendError {
    BackendError::native(format!(
        "Windows MIDI {operation} failed (mmsyserr {status})"
    ))
}

impl GraphDriver for WindowsMidiDriver {
    fn capabilities(&self) -> BackendCapabilities {
        WINDOWS_MIDI_CAPABILITIES
    }

    fn refresh(&mut self) -> BackendResult<Vec<Node>> {
        let devices = Self::enumerate();
        let mut graph = Graph::default();
        let mut map = BTreeMap::new();
        for device in devices {
            graph.add_node(Node::new(
                device.node_id,
                device.name.clone(),
                pw_graph_core::NodeType::WindowsMidi,
            ))?;
            graph.add_port(Port::new(
                device.port_id,
                device.node_id,
                "midi",
                match device.kind {
                    // An input device is a *source* of MIDI for the graph.
                    DeviceKind::Input => Direction::Source,
                    DeviceKind::Output => Direction::Sink,
                },
                PortType::MidiJack,
            ))?;
            map.insert(device.port_id, device);
        }
        // Connections this process opened survive a refresh, so they are
        // carried into the rebuilt graph. One whose device has been unplugged
        // is dropped, which closes its handles with it.
        let mut surviving = Vec::with_capacity(self.connections.len());
        for connection in std::mem::take(&mut self.connections) {
            let link = self.graph.link(connection.link_id).cloned();
            let still_present = link.as_ref().is_some_and(|link| {
                map.contains_key(&link.output_port) && map.contains_key(&link.input_port)
            });
            match (still_present, link) {
                (true, Some(link)) => {
                    let _ = graph.insert_existing_link(link);
                    surviving.push(connection);
                }
                // Dropping `connection` here closes both handles.
                _ => continue,
            }
        }
        self.connections = surviving;
        for (node_id, position) in graph.default_node_positions() {
            if let Some(node) = graph.nodes.get_mut(&node_id) {
                node.position = self.positions.get(&node_id).copied().unwrap_or(position);
            }
        }
        self.graph = graph;
        self.devices = map;
        Ok(self.graph.nodes.values().cloned().collect())
    }

    fn connect(&mut self, src: PortId, dst: PortId) -> BackendResult<Link> {
        let (source, destination) = self.endpoints(src, dst)?;
        // Windows routes one input to one output without a thru driver, so a
        // second connection from the same input is refused rather than
        // silently replacing the first.
        if self
            .connections
            .iter()
            .filter_map(|connection| self.graph.link(connection.link_id))
            .any(|link| link.output_port == src)
        {
            return Err(BackendError::unsupported(
                "this MIDI input already drives an output; Windows routes one at a time",
            ));
        }

        let mut input = Audio::HMIDIIN::default();
        let status = unsafe {
            Audio::midiInOpen(
                &mut input,
                source.index,
                None,
                None,
                Audio::MIDI_WAVE_OPEN_TYPE(0),
            )
        };
        if status != MM_OK {
            return Err(mm_error("input open", status));
        }
        let mut output = Audio::HMIDIOUT::default();
        let status = unsafe {
            Audio::midiOutOpen(
                &mut output,
                destination.index,
                None,
                None,
                Audio::MIDI_WAVE_OPEN_TYPE(0),
            )
        };
        if status != MM_OK {
            let _ = unsafe { Audio::midiInClose(input) };
            return Err(mm_error("output open", status));
        }
        let status = unsafe { Audio::midiConnect(Audio::HMIDI(input.0), output, None) };
        if status != MM_OK {
            let _ = unsafe { Audio::midiOutClose(output) };
            let _ = unsafe { Audio::midiInClose(input) };
            return Err(mm_error("connect", status));
        }
        // Nothing flows until the input is started.
        let status = unsafe { Audio::midiInStart(input) };
        if status != MM_OK {
            let _ = unsafe { Audio::midiDisconnect(Audio::HMIDI(input.0), output, None) };
            let _ = unsafe { Audio::midiOutClose(output) };
            let _ = unsafe { Audio::midiInClose(input) };
            return Err(mm_error("input start", status));
        }

        let link_id = self.allocate_link();
        let link = self.graph.add_link(link_id, src, dst)?;
        self.connections.push(OpenConnection {
            input,
            output,
            link_id,
        });
        Ok(link)
    }

    fn disconnect(&mut self, link: LinkId) -> BackendResult<Link> {
        let position = self
            .connections
            .iter()
            .position(|connection| connection.link_id == link)
            .ok_or(GraphError::MissingLink(link))?;
        // Dropping the connection disconnects and closes both handles.
        drop(self.connections.remove(position));
        Ok(self.graph.remove_link(link)?)
    }

    fn set_node_position(&mut self, node: NodeId, position: [f32; 2]) -> BackendResult<()> {
        self.graph
            .nodes
            .get_mut(&node)
            .ok_or(GraphError::MissingNode(node))?
            .position = position;
        self.positions.insert(node, position);
        Ok(())
    }

    fn graph(&self) -> &Graph {
        &self.graph
    }

    fn is_node_type(&self, node_type: pw_graph_core::NodeType) -> bool {
        node_type == pw_graph_core::NodeType::WindowsMidi
    }

    fn is_port_type(&self, port_type: PortType) -> bool {
        port_type == PortType::MidiJack
    }

    fn is_link_mutable(&self, link: LinkId) -> bool {
        self.graph.link(link).is_some()
    }
}

impl EffectDriver for WindowsMidiDriver {}

#[cfg(feature = "relay")]
impl crate::api::RelayDriver for WindowsMidiDriver {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_names_stop_at_the_nul_padding() {
        let mut raw = [0u16; 32];
        for (slot, value) in raw.iter_mut().zip("MPK mini".encode_utf16()) {
            *slot = value;
        }
        assert_eq!(wide_name(&raw), "MPK mini");
        assert_eq!(wide_name(&[0u16; 8]), "");
    }

    /// Inputs and outputs are numbered separately by WinMM, so the kind has to
    /// be part of the id or input 0 and output 0 would collide.
    #[test]
    fn inputs_and_outputs_never_share_an_id() {
        let input = device_from_caps(0, DeviceKind::Input, "in".into());
        let output = device_from_caps(0, DeviceKind::Output, "out".into());
        assert_ne!(input.node_id, output.node_id);
        assert_ne!(input.port_id, output.port_id);
        // And a node never collides with a port.
        assert_ne!(input.node_id.0, input.port_id.0);
    }

    #[test]
    fn device_identity_does_not_change_when_winmm_reorders_indices() {
        let first = device_from_identity(
            0,
            DeviceKind::Input,
            "keyboard".into(),
            "\\\\?\\midi#{stable-key}",
        );
        let reordered = device_from_identity(
            7,
            DeviceKind::Input,
            "keyboard".into(),
            "\\\\?\\midi#{stable-key}",
        );
        let different = device_from_identity(
            0,
            DeviceKind::Input,
            "keyboard".into(),
            "\\\\?\\midi#{other-key}",
        );

        assert_eq!(first.node_id, reordered.node_id);
        assert_eq!(first.port_id, reordered.port_id);
        assert_ne!(first.node_id, different.node_id);
        assert_ne!(first.port_id, different.port_id);
    }

    #[test]
    fn fallback_identity_includes_direction_and_device_description() {
        let input = fallback_identity(DeviceKind::Input, "same", 1, 2, 3);
        let output = fallback_identity(DeviceKind::Output, "same", 1, 2, 3);
        assert_ne!(input, output);
        assert_ne!(
            input,
            fallback_identity(DeviceKind::Input, "other", 1, 2, 3)
        );
    }

    #[test]
    fn identity_occurrences_are_scoped_to_each_direction() {
        let mut identities = BTreeMap::new();
        assert_eq!(
            unique_identity(DeviceKind::Input, "same-interface".into(), &mut identities),
            "same-interface"
        );
        assert_eq!(
            unique_identity(DeviceKind::Output, "same-interface".into(), &mut identities),
            "same-interface"
        );
        assert_eq!(
            unique_identity(DeviceKind::Input, "same-interface".into(), &mut identities),
            "same-interface#2"
        );
    }

    #[test]
    fn every_id_lands_in_the_windows_midi_namespace() {
        let device = device_from_caps(3, DeviceKind::Output, "synth".into());
        assert_eq!(
            pw_graph_core::backend_for_node(device.node_id),
            Some(pw_graph_core::BackendKind::WindowsMidi)
        );
        assert_eq!(
            pw_graph_core::backend_for_port(device.port_id),
            Some(pw_graph_core::BackendKind::WindowsMidi)
        );
    }

    /// MIDI is the one Windows backend that can rewire itself, and WinMM is
    /// always present, so this runs anywhere Windows does.
    #[test]
    fn windows_midi_reports_real_routing() {
        let driver = WindowsMidiDriver::new().expect("WinMM is part of Windows");
        let capabilities = driver.capabilities();

        assert!(capabilities.connect, "midiConnect is real routing");
        assert!(capabilities.disconnect);
        // ...and none of the audio facilities, which it genuinely lacks.
        assert!(!capabilities.volume);
        assert!(!capabilities.meters);
    }

    /// Enumeration must not open anything: opening a device would take it from
    /// whatever application is using it just to draw the graph.
    #[test]
    fn listing_devices_opens_no_handles() {
        let driver = WindowsMidiDriver::new().expect("WinMM is part of Windows");

        assert!(driver.connections.is_empty());
        // Every listed device still gets a node and exactly one port.
        for node in driver.graph().nodes.values() {
            assert_eq!(node.ports.len(), 1, "{} has one MIDI port", node.name);
        }
        assert_eq!(driver.devices.len(), driver.graph().ports.len());
    }

    /// A MIDI link only runs input to output; anything else is refused rather
    /// than handed to WinMM.
    #[test]
    fn a_connection_must_run_from_an_input_to_an_output() {
        let mut driver = WindowsMidiDriver::default();
        let input = device_from_caps(0, DeviceKind::Input, "keys".into());
        let output = device_from_caps(0, DeviceKind::Output, "synth".into());
        driver.devices.insert(input.port_id, input.clone());
        driver.devices.insert(output.port_id, output.clone());

        assert!(driver.endpoints(input.port_id, output.port_id).is_ok());
        for (src, dst) in [
            (output.port_id, input.port_id),
            (input.port_id, input.port_id),
            (output.port_id, output.port_id),
        ] {
            assert!(
                matches!(
                    driver.endpoints(src, dst),
                    Err(BackendError::Unsupported(_))
                ),
                "a MIDI link runs input to output only"
            );
        }
    }
}
