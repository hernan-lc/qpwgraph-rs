//! Backend snapshots and the node-level controls implemented by the Slint UI.

use crate::args::Args;
use pw_graph_backend::{
    AudioMeter, DemoDriver, EffectDriver, EffectInsertRequest, EffectInstance, EffectNodeRequest,
    GraphDriver, MeterPolicy,
};
#[cfg(feature = "relay")]
use pw_graph_backend::{
    RelayDriver, RelayEngineStatus, RelayEvent, RelayHostRequest, RelayLocalLink, RelayPeerInfo,
    RelayRoles, RelaySessionId,
};
#[cfg(any(feature = "pipewire", feature = "alsa"))]
use pw_graph_core::GraphError;
use pw_graph_core::{Graph, NodeId, PortKey, PortType};
use pw_graph_effects::EffectDescriptor;
use std::collections::BTreeSet;
use std::time::Instant;

#[cfg(feature = "alsa")]
use pw_graph_alsamidi::AlsaMidiDriver;
#[cfg(feature = "pipewire")]
use pw_graph_backend::PipewireDriver;

pub(crate) struct ReadOnlyGraphSource {
    graph: Graph,
    backend_name: String,
    meter_policy: MeterPolicy,
    meter_epoch: Instant,
    demo: Option<DemoDriver>,
    #[cfg(feature = "pipewire")]
    pipewire: Option<PipewireDriver>,
    #[cfg(feature = "alsa")]
    alsa: Option<AlsaMidiDriver>,
}

impl ReadOnlyGraphSource {
    pub(crate) fn new(args: &Args, meter_policy: MeterPolicy) -> (Self, String) {
        if args.demo {
            let mut demo = DemoDriver::demo();
            let graph = demo.graph().clone();
            let _ = demo.refresh();
            return (
                Self {
                    graph,
                    backend_name: "demo".into(),
                    meter_policy,
                    meter_epoch: Instant::now(),
                    demo: Some(demo),
                    #[cfg(feature = "pipewire")]
                    pipewire: None,
                    #[cfg(feature = "alsa")]
                    alsa: None,
                },
                "Slint UI connected to deterministic demo data".into(),
            );
        }

        #[cfg(feature = "pipewire")]
        let (pipewire, pipewire_error) = match PipewireDriver::new() {
            Ok(driver) => (Some(driver), None),
            Err(error) => (None, Some(error.to_string())),
        };

        #[cfg(feature = "alsa")]
        let (alsa, alsa_error) = if args.no_alsa_midi {
            (None, None)
        } else {
            match AlsaMidiDriver::new() {
                Ok(driver) => (Some(driver), None),
                Err(error) => (None, Some(error.to_string())),
            }
        };

        #[cfg(not(feature = "pipewire"))]
        let pipewire_error: Option<String> = None;
        #[cfg(not(feature = "alsa"))]
        let alsa_error: Option<String> = None;

        #[allow(unused_mut)]
        let mut backend_names: Vec<&str> = Vec::new();
        #[cfg(feature = "pipewire")]
        if pipewire.is_some() {
            backend_names.push("pipewire");
        }
        #[cfg(feature = "alsa")]
        if alsa.is_some() {
            backend_names.push("alsa");
        }
        let backend_name = if backend_names.is_empty() {
            "none".into()
        } else {
            backend_names.join("+")
        };

        let mut source = Self {
            graph: Graph::default(),
            backend_name,
            meter_policy,
            meter_epoch: Instant::now(),
            demo: None,
            #[cfg(feature = "pipewire")]
            pipewire,
            #[cfg(feature = "alsa")]
            alsa,
        };
        let status = match source.refresh() {
            Ok(()) if !source.graph.nodes.is_empty() => {
                "Slint UI connected to the live graph".into()
            }
            Ok(()) => "No live graph is available; use --demo for a preview graph".into(),
            Err(error) => format!("Could not refresh live graph: {error}"),
        };

        let meter_error = source.set_meter_policy(meter_policy).err();

        let failures: Vec<_> = [pipewire_error, alsa_error].into_iter().flatten().collect();
        let status = if failures.is_empty() {
            status
        } else {
            format!("{status} · {}", failures.join(" · "))
        };
        let status = match meter_error {
            Some(error) => format!("{status} · Meter setup unavailable: {error}"),
            None => status,
        };
        (source, status)
    }

    pub(crate) fn graph(&self) -> &Graph {
        &self.graph
    }

    pub(crate) fn backend_name(&self) -> &str {
        &self.backend_name
    }

    pub(crate) fn graph_dirty(&self) -> bool {
        #[allow(unused_mut)]
        let mut dirty = false;
        #[cfg(feature = "pipewire")]
        {
            dirty |= self
                .pipewire
                .as_ref()
                .is_some_and(|driver| driver.graph_dirty());
        }
        #[cfg(feature = "alsa")]
        {
            dirty |= self
                .alsa
                .as_ref()
                .is_some_and(|driver| driver.graph_dirty());
        }
        self.demo.is_none() && dirty
    }

    pub(crate) fn meter_policy(&self) -> MeterPolicy {
        self.meter_policy
    }

    pub(crate) fn is_demo(&self) -> bool {
        self.demo.is_some()
    }

    pub(crate) fn has_meter_backend(&self) -> bool {
        #[cfg(feature = "pipewire")]
        let has_pipewire = self.pipewire.is_some();
        #[cfg(not(feature = "pipewire"))]
        let has_pipewire = false;
        has_pipewire
    }

    pub(crate) fn request_meters(
        &mut self,
        _requested_nodes: &BTreeSet<NodeId>,
    ) -> Result<(), String> {
        if self.meter_policy == MeterPolicy::Disabled || self.demo.is_some() {
            return Ok(());
        }
        #[cfg(feature = "pipewire")]
        if let Some(driver) = self.pipewire.as_mut() {
            driver
                .request_meters(_requested_nodes)
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    pub(crate) fn audio_meters(&mut self) -> Result<Vec<AudioMeter>, String> {
        if self.demo.is_some() {
            return Ok(self.demo_meters());
        }
        #[cfg(feature = "pipewire")]
        if let Some(driver) = self.pipewire.as_mut() {
            return driver.audio_meters().map_err(|error| error.to_string());
        }
        Ok(Vec::new())
    }

    pub(crate) fn reset_meters(&mut self) {
        #[cfg(feature = "pipewire")]
        if let Some(driver) = self.pipewire.as_mut() {
            let _ = driver.reset_audio_config();
        }
    }

    pub(crate) fn refresh(&mut self) -> Result<(), String> {
        if let Some(driver) = self.demo.as_mut() {
            driver.refresh().map_err(|error| error.to_string())?;
            self.graph = driver.graph().clone();
            return Ok(());
        }

        #[allow(unused_mut)]
        let mut graph = Graph::default();
        #[cfg(feature = "pipewire")]
        if let Some(driver) = self.pipewire.as_mut() {
            driver.refresh().map_err(|error| error.to_string())?;
            merge_graph(&mut graph, driver.graph()).map_err(|error| error.to_string())?;
        }
        #[cfg(feature = "alsa")]
        if let Some(driver) = self.alsa.as_mut() {
            driver.refresh().map_err(|error| error.to_string())?;
            merge_graph(&mut graph, driver.graph()).map_err(|error| error.to_string())?;
        }
        self.graph = graph;
        Ok(())
    }

    pub(crate) fn set_meter_policy(&mut self, policy: MeterPolicy) -> Result<(), String> {
        self.meter_policy = policy;
        #[cfg(feature = "pipewire")]
        if let Some(driver) = self.pipewire.as_mut() {
            driver
                .set_meter_policy(policy)
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    pub(crate) fn set_node_volume(&mut self, node: NodeId, volume: f32) -> Result<(), String> {
        if let Some(driver) = self.demo.as_mut() {
            return driver
                .set_node_volume(node, volume)
                .map_err(|error| error.to_string());
        }
        #[cfg(feature = "pipewire")]
        if let Some(driver) = self.pipewire.as_mut() {
            return driver
                .set_node_volume(node, volume)
                .map_err(|error| error.to_string());
        }
        Err("this node is not controlled by an audio backend".into())
    }

    pub(crate) fn set_node_mute(&mut self, node: NodeId, muted: bool) -> Result<(), String> {
        if let Some(driver) = self.demo.as_mut() {
            return driver
                .set_node_mute(node, muted)
                .map_err(|error| error.to_string());
        }
        #[cfg(feature = "pipewire")]
        if let Some(driver) = self.pipewire.as_mut() {
            return driver
                .set_node_mute(node, muted)
                .map_err(|error| error.to_string());
        }
        Err("this node is not controlled by an audio backend".into())
    }

    pub(crate) fn effect_descriptors(&self) -> Vec<EffectDescriptor> {
        if let Some(driver) = self.demo.as_ref() {
            return driver.effect_descriptors();
        }
        #[cfg(feature = "pipewire")]
        if let Some(driver) = self.pipewire.as_ref() {
            return driver.effect_descriptors();
        }
        pw_graph_effects::EffectHost::new().descriptors()
    }

    pub(crate) fn effect_instances(&self) -> Vec<EffectInstance> {
        if let Some(driver) = self.demo.as_ref() {
            return driver.effect_instances();
        }
        #[cfg(feature = "pipewire")]
        if let Some(driver) = self.pipewire.as_ref() {
            return driver.effect_instances();
        }
        Vec::new()
    }

    pub(crate) fn supports_effect_nodes(&self) -> bool {
        if let Some(driver) = self.demo.as_ref() {
            return driver.supports_effect_nodes();
        }
        #[cfg(feature = "pipewire")]
        if let Some(driver) = self.pipewire.as_ref() {
            return driver.supports_effect_nodes();
        }
        false
    }

    pub(crate) fn create_effect_node(
        &mut self,
        request: EffectNodeRequest,
    ) -> Result<EffectInstance, String> {
        if let Some(driver) = self.demo.as_mut() {
            return driver
                .create_effect_node(request)
                .map_err(|error| error.to_string());
        }
        #[cfg(feature = "pipewire")]
        if let Some(driver) = self.pipewire.as_mut() {
            return driver
                .create_effect_node(request)
                .map_err(|error| error.to_string());
        }
        Err("effect processing is not available for this backend".into())
    }

    pub(crate) fn insert_effect(
        &mut self,
        request: EffectInsertRequest,
    ) -> Result<EffectInstance, String> {
        if let Some(driver) = self.demo.as_mut() {
            return driver
                .insert_effect(request)
                .map_err(|error| error.to_string());
        }
        #[cfg(feature = "pipewire")]
        if let Some(driver) = self.pipewire.as_mut() {
            return driver
                .insert_effect(request)
                .map_err(|error| error.to_string());
        }
        Err("effect processing is not available for this backend".into())
    }

    pub(crate) fn set_effect_enabled(
        &mut self,
        instance_id: &str,
        enabled: bool,
    ) -> Result<(), String> {
        if let Some(driver) = self.demo.as_mut() {
            return driver
                .set_effect_enabled(instance_id, enabled)
                .map_err(|error| error.to_string());
        }
        #[cfg(feature = "pipewire")]
        if let Some(driver) = self.pipewire.as_mut() {
            return driver
                .set_effect_enabled(instance_id, enabled)
                .map_err(|error| error.to_string());
        }
        Err("effect processing is not available for this backend".into())
    }

    pub(crate) fn set_effect_parameter(
        &mut self,
        instance_id: &str,
        parameter: &str,
        value: f32,
    ) -> Result<(), String> {
        if let Some(driver) = self.demo.as_mut() {
            return driver
                .set_effect_parameter(instance_id, parameter, value)
                .map_err(|error| error.to_string());
        }
        #[cfg(feature = "pipewire")]
        if let Some(driver) = self.pipewire.as_mut() {
            return driver
                .set_effect_parameter(instance_id, parameter, value)
                .map_err(|error| error.to_string());
        }
        Err("effect processing is not available for this backend".into())
    }

    pub(crate) fn remove_effect(&mut self, instance_id: &str) -> Result<(), String> {
        if let Some(driver) = self.demo.as_mut() {
            return driver
                .remove_effect(instance_id)
                .map_err(|error| error.to_string());
        }
        #[cfg(feature = "pipewire")]
        if let Some(driver) = self.pipewire.as_mut() {
            return driver
                .remove_effect(instance_id)
                .map_err(|error| error.to_string());
        }
        Err("effect processing is not available for this backend".into())
    }

    pub(crate) fn connect_by_key_if_missing(
        &mut self,
        output: &PortKey,
        input: &PortKey,
    ) -> Result<bool, String> {
        if let Some(driver) = self.demo.as_mut() {
            return driver
                .connect_by_key_if_missing(output, input)
                .map(|link| link.is_some())
                .map_err(|error| error.to_string());
        }
        #[cfg(feature = "pipewire")]
        if let Some(driver) = self.pipewire.as_mut() {
            return driver
                .connect_by_key_if_missing(output, input)
                .map(|link| link.is_some())
                .map_err(|error| error.to_string());
        }
        Err("graph connections are not available for this backend".into())
    }

    #[cfg(feature = "relay")]
    pub(crate) fn relay_available(&self) -> bool {
        if let Some(driver) = self.demo.as_ref() {
            return driver.relay_available();
        }
        self.pipewire
            .as_ref()
            .is_some_and(|driver| driver.relay_available())
    }

    #[cfg(feature = "relay")]
    pub(crate) fn relay_status(&self) -> RelayEngineStatus {
        if let Some(driver) = self.demo.as_ref() {
            return driver.relay_status();
        }
        self.pipewire
            .as_ref()
            .map(|driver| driver.relay_status())
            .unwrap_or_default()
    }

    #[cfg(feature = "relay")]
    pub(crate) fn relay_start_host(&mut self, request: RelayHostRequest) -> Result<u16, String> {
        if let Some(driver) = self.demo.as_mut() {
            return driver
                .relay_start_host(request)
                .map_err(|error| error.to_string());
        }
        self.pipewire
            .as_mut()
            .ok_or_else(|| "audio relay is not available for this backend".to_owned())?
            .relay_start_host(request)
            .map_err(|error| error.to_string())
    }

    #[cfg(feature = "relay")]
    pub(crate) fn relay_stop_host(&mut self) -> Result<(), String> {
        if let Some(driver) = self.demo.as_mut() {
            return driver.relay_stop_host().map_err(|error| error.to_string());
        }
        self.pipewire
            .as_mut()
            .ok_or_else(|| "audio relay is not available for this backend".to_owned())?
            .relay_stop_host()
            .map_err(|error| error.to_string())
    }

    #[cfg(feature = "relay")]
    pub(crate) fn relay_connect(
        &mut self,
        target: std::net::SocketAddr,
        pin: &str,
        roles: RelayRoles,
    ) -> Result<(), String> {
        if let Some(driver) = self.demo.as_mut() {
            return driver
                .relay_connect(target, pin, roles)
                .map_err(|error| error.to_string());
        }
        self.pipewire
            .as_mut()
            .ok_or_else(|| "audio relay is not available for this backend".to_owned())?
            .relay_connect(target, pin, roles)
            .map_err(|error| error.to_string())
    }

    #[cfg(feature = "relay")]
    pub(crate) fn relay_disconnect(&mut self, session: RelaySessionId) -> Result<(), String> {
        if let Some(driver) = self.demo.as_mut() {
            return driver
                .relay_disconnect(session)
                .map_err(|error| error.to_string());
        }
        self.pipewire
            .as_mut()
            .ok_or_else(|| "audio relay is not available for this backend".to_owned())?
            .relay_disconnect(session)
            .map_err(|error| error.to_string())
    }

    #[cfg(feature = "relay")]
    pub(crate) fn relay_events(&mut self) -> Vec<RelayEvent> {
        if let Some(driver) = self.demo.as_mut() {
            return driver.relay_events();
        }
        self.pipewire
            .as_mut()
            .map(|driver| driver.relay_events())
            .unwrap_or_default()
    }

    #[cfg(feature = "relay")]
    pub(crate) fn relay_discovery_start(&mut self) -> Result<(), String> {
        if let Some(driver) = self.demo.as_mut() {
            return driver
                .relay_discovery_start()
                .map_err(|error| error.to_string());
        }
        self.pipewire
            .as_mut()
            .ok_or_else(|| "audio relay is not available for this backend".to_owned())?
            .relay_discovery_start()
            .map_err(|error| error.to_string())
    }

    #[cfg(feature = "relay")]
    pub(crate) fn relay_discovery_stop(&mut self) {
        if let Some(driver) = self.demo.as_mut() {
            driver.relay_discovery_stop();
        }
        if let Some(driver) = self.pipewire.as_mut() {
            driver.relay_discovery_stop();
        }
    }

    #[cfg(feature = "relay")]
    pub(crate) fn relay_peers(&self) -> Vec<RelayPeerInfo> {
        if let Some(driver) = self.demo.as_ref() {
            return driver.relay_peers();
        }
        self.pipewire
            .as_ref()
            .map(|driver| driver.relay_peers())
            .unwrap_or_default()
    }

    #[cfg(feature = "relay")]
    pub(crate) fn relay_local_links(&self) -> Vec<RelayLocalLink> {
        if let Some(driver) = self.demo.as_ref() {
            return driver.relay_local_links();
        }
        self.pipewire
            .as_ref()
            .map(|driver| driver.relay_local_links())
            .unwrap_or_default()
    }

    fn demo_meters(&self) -> Vec<AudioMeter> {
        if self.meter_policy == MeterPolicy::Disabled {
            return Vec::new();
        }
        let elapsed = self.meter_epoch.elapsed().as_secs_f32();
        self.graph
            .nodes
            .values()
            .filter(|node| {
                node.ports.iter().any(|port_id| {
                    self.graph
                        .port(*port_id)
                        .is_some_and(|port| port.port_type == PortType::Audio)
                })
            })
            .map(|node| {
                let phase =
                    elapsed * (1.3 + (node.id.0 % 5) as f32 * 0.19) + (node.id.0 % 17) as f32;
                let rms = (0.07 + phase.sin().abs() * 0.58).clamp(0.0, 1.0);
                let peak = (rms + 0.12 + (phase * 1.7).sin().abs() * 0.18).clamp(0.0, 1.0);
                AudioMeter {
                    node_id: node.id,
                    port_id: None,
                    rms,
                    peak,
                    age_ms: 0,
                    available: true,
                }
            })
            .collect()
    }
}

#[cfg(any(feature = "pipewire", feature = "alsa"))]
fn merge_graph(destination: &mut Graph, source: &Graph) -> Result<(), GraphError> {
    for node in source.nodes.values().cloned() {
        destination.add_node(node)?;
    }
    for port in source.ports.values().cloned() {
        destination.add_port(port)?;
    }
    for link in source.links.values().cloned() {
        destination.insert_existing_link(link)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo_args() -> Args {
        Args {
            demo: true,
            ..Args::default()
        }
    }

    #[test]
    fn demo_source_supplies_live_style_audio_readings() {
        let (mut source, _) = ReadOnlyGraphSource::new(&demo_args(), MeterPolicy::OnDemand);

        let readings = source.audio_meters().unwrap();

        assert!(!readings.is_empty());
        assert!(readings.iter().all(|reading| {
            reading.available
                && (0.0..=1.0).contains(&reading.rms)
                && (reading.rms..=1.0).contains(&reading.peak)
        }));
    }

    #[test]
    fn disabled_demo_source_never_reports_a_level() {
        let (mut source, _) = ReadOnlyGraphSource::new(&demo_args(), MeterPolicy::Disabled);

        assert!(source.audio_meters().unwrap().is_empty());
    }

    #[test]
    fn demo_source_accepts_node_audio_controls() {
        let (mut source, _) = ReadOnlyGraphSource::new(&demo_args(), MeterPolicy::Disabled);
        let node = *source.graph().nodes.keys().next().unwrap();

        source.set_node_volume(node, 0.42).unwrap();
        source.set_node_mute(node, true).unwrap();
    }

    #[test]
    fn demo_source_connects_stable_ports_and_is_idempotent() {
        let (mut source, _) = ReadOnlyGraphSource::new(&demo_args(), MeterPolicy::Disabled);
        let output = source.graph().port_key(pw_graph_core::PortId(1)).unwrap();
        let input = source.graph().port_key(pw_graph_core::PortId(3)).unwrap();

        assert!(source.connect_by_key_if_missing(&output, &input).unwrap());
        assert!(!source.connect_by_key_if_missing(&output, &input).unwrap());
        source.refresh().unwrap();

        assert!(source
            .graph()
            .links
            .values()
            .any(|link| link.output_port.0 == 1 && link.input_port.0 == 3));
    }

    #[test]
    fn demo_source_manages_effect_nodes() {
        let (mut source, _) = ReadOnlyGraphSource::new(&demo_args(), MeterPolicy::Disabled);
        let descriptor = source.effect_descriptors().into_iter().next().unwrap();
        let parameters = descriptor
            .parameters
            .iter()
            .map(|parameter| (parameter.id.clone(), parameter.default))
            .collect();
        let instance = source
            .create_effect_node(EffectNodeRequest {
                instance_id: "slint-test-effect".into(),
                effect_id: descriptor.id,
                module_path: None,
                enabled: true,
                parameters,
                position: [260.0, 180.0],
            })
            .unwrap();

        assert_eq!(source.effect_instances().len(), 1);
        source
            .set_effect_enabled(&instance.config.instance_id, false)
            .unwrap();
        assert!(!source.effect_instances()[0].config.enabled);
        source.remove_effect(&instance.config.instance_id).unwrap();
        source.refresh().unwrap();
        assert!(source.effect_instances().is_empty());
        assert!(source.graph().node(instance.node_id).is_none());
    }
}
