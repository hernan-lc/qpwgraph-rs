//! Backend abstraction.
//!
//! The crate root is intentionally a small compatibility façade. Shared
//! contracts live in [`api`], the deterministic implementation lives in
//! [`demo`], and the native implementation is isolated in [`pipewire`].

mod api;
mod demo;
#[cfg(feature = "pipewire")]
mod pipewire;
#[cfg(not(feature = "pipewire"))]
mod pipewire_stub;

pub use api::*;
pub use demo::{DemoDriver, InMemoryDriver};

#[cfg(feature = "pipewire")]
pub use pipewire::PipewireDriver;
#[cfg(not(feature = "pipewire"))]
pub use pipewire_stub::PipewireDriver;

#[cfg(feature = "pipewire")]
impl EffectDriver for PipewireDriver {}

// The native driver and its focused submodules use these graph types in their
// internal implementation. Keep the imports at the façade boundary so those
// modules do not need to depend on the public API module's implementation
// details.
#[allow(unused_imports)]
pub(crate) use pw_graph_core::{
    Direction, Graph, GraphError, Link, LinkId, Node, NodeId, NodeType, Port, PortId, PortKey,
    PortType,
};

use std::collections::BTreeSet;

/// Used by patchbay activation to avoid reconnecting identical links.
pub fn existing_connections(driver: &dyn GraphDriver) -> BTreeSet<(PortId, PortId)> {
    driver
        .graph()
        .links
        .values()
        .map(|link| (link.output_port, link.input_port))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "pipewire")]
    use pw_graph_core::PortType;
    use pw_graph_core::{NodeType, PortId};
    use std::collections::BTreeMap;
    #[cfg(feature = "pipewire")]
    use std::collections::BTreeSet;

    #[test]
    fn meter_policy_round_trips_and_defaults_safely() {
        for policy in MeterPolicy::ALL {
            assert_eq!(MeterPolicy::parse(policy.as_str()), policy);
        }
        assert_eq!(MeterPolicy::parse("OFF"), MeterPolicy::Disabled);
        assert_eq!(MeterPolicy::parse("all"), MeterPolicy::Always);
        // An unreadable or older config must not silently start metering
        // everything, so anything unrecognized lands on the default.
        assert_eq!(MeterPolicy::parse("nonsense"), MeterPolicy::default());
        assert_eq!(MeterPolicy::default(), MeterPolicy::OnDemand);
    }

    #[test]
    fn demo_backend_connects_and_disconnects() {
        let mut driver = DemoDriver::demo();
        let link = driver.connect(PortId(1), PortId(3)).unwrap();
        assert_eq!(driver.graph().links.len(), 1);
        driver.disconnect(link.id).unwrap();
        assert!(driver.graph().links.is_empty());
    }

    #[test]
    fn demo_backend_has_a_stable_graph_for_demo_runs() {
        let driver = DemoDriver::demo();
        assert_eq!(driver.graph().nodes.len(), 4);
        assert_eq!(driver.graph().ports.len(), 6);
        assert!(driver
            .graph()
            .nodes
            .values()
            .all(|node| node.node_type == NodeType::PipeWire));
    }

    #[test]
    fn demo_backend_inserts_and_removes_an_effect_transactionally() {
        let mut driver = DemoDriver::demo();
        driver.connect(PortId(1), PortId(3)).unwrap();
        let source = driver.graph().port_key(PortId(1)).unwrap();
        let destination = driver.graph().port_key(PortId(3)).unwrap();
        let instance = driver
            .insert_effect(EffectInsertRequest {
                instance_id: "test-effect".into(),
                effect_id: pw_graph_effects::NOISE_GATE_ID.into(),
                module_path: None,
                source,
                destination,
                enabled: true,
                parameters: BTreeMap::new(),
            })
            .unwrap();
        assert_eq!(driver.effect_instances().len(), 1);
        assert_eq!(
            driver.graph().nodes[&instance.node_id].node_type,
            NodeType::Effect
        );
        assert_eq!(driver.graph().links.len(), 2);
        driver.remove_effect("test-effect").unwrap();
        assert!(driver.effect_instances().is_empty());
        assert_eq!(driver.graph().links.len(), 1);
        assert_eq!(driver.graph().nodes.len(), 4);
    }

    #[cfg(feature = "pipewire")]
    #[test]
    fn native_backend_refreshes_running_pipewire_registry() {
        let Ok(mut driver) = PipewireDriver::new() else {
            // CI and development containers may not have a user PipeWire
            // daemon. The live test is exercised automatically when one is
            // available, but should not make offline builds fail.
            return;
        };
        let nodes = driver
            .refresh()
            .expect("PipeWire registry snapshot should succeed");
        assert!(!nodes.is_empty());
        assert!(!driver.graph().ports.is_empty());
    }

    /// Regression guard for the startup behaviour users actually noticed: the
    /// driver used to open a capture stream against every audio node as soon
    /// as the graph was first read, which resumed suspended devices and made
    /// the daemon renegotiate their format.
    #[cfg(feature = "pipewire")]
    #[test]
    fn native_backend_meters_nothing_until_it_is_asked_to() {
        let Ok(mut driver) = PipewireDriver::new() else {
            return;
        };
        driver.refresh().expect("registry snapshot should succeed");
        assert_eq!(driver.active_meter_count(), 0);
        assert!(driver.audio_meters().unwrap().is_empty());
    }

    /// Opt-in: this one attaches a real (passive, monitor-flagged) stream to a
    /// node in the user's live session, so it is not part of a default run.
    #[cfg(feature = "pipewire")]
    #[test]
    fn native_backend_attaches_and_releases_a_requested_meter() {
        if std::env::var_os("PW_GRAPH_TEST_METERS").is_none() {
            return;
        }
        let mut driver = PipewireDriver::new().expect("PipeWire daemon should be available");
        driver.refresh().expect("registry snapshot should succeed");
        let target = driver.graph().nodes.values().find(|node| {
            node.ports.iter().any(|port_id| {
                driver.graph().port(*port_id).is_some_and(|port| {
                    port.direction.is_source() && port.port_type == PortType::Audio
                })
            })
        });
        let Some(target) = target.map(|node| node.id) else {
            return;
        };

        driver
            .request_meters(&BTreeSet::from([target]))
            .expect("requesting a meter should succeed");
        assert_eq!(driver.active_meter_count(), 1);

        // Regression guard: `process` runs on PipeWire's realtime data thread,
        // which the thread-loop lock does not exclude. Reading meters from
        // this thread while that thread publishes used to hit `RefCell already
        // borrowed` inside a callback that cannot unwind, aborting the
        // process. Polling hard for a second reliably reproduced it.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        let mut polls = 0_u32;
        while std::time::Instant::now() < deadline {
            for meter in driver
                .audio_meters()
                .expect("reading meters should succeed")
            {
                assert!(meter.rms.is_finite() && (0.0..=1.0).contains(&meter.rms));
                assert!(meter.peak.is_finite() && (0.0..=1.0).contains(&meter.peak));
            }
            polls += 1;
        }
        assert!(polls > 0);

        driver
            .reset_audio_config()
            .expect("releasing meters should succeed");
        assert_eq!(driver.active_meter_count(), 0);
        assert!(driver.audio_meters().unwrap().is_empty());
    }

    #[cfg(feature = "pipewire")]
    #[test]
    fn native_backend_can_create_and_destroy_a_link_when_enabled() {
        if std::env::var_os("PW_GRAPH_TEST_LINKS").is_none() {
            return;
        }
        let mut driver = PipewireDriver::new().expect("PipeWire daemon should be available");
        driver
            .refresh()
            .expect("PipeWire registry snapshot should succeed");
        let existing = existing_connections(&driver);
        let pair = driver.graph().ports.values().find_map(|output| {
            if !output.direction.is_source() {
                return None;
            }
            driver.graph().ports.values().find_map(|input| {
                if !input.direction.is_sink()
                    || (output.port_type != input.port_type
                        && output.port_type != PortType::Unknown
                        && input.port_type != PortType::Unknown)
                    || existing.contains(&(output.id, input.id))
                {
                    return None;
                }
                Some((output.id, input.id))
            })
        });
        let Some((output, input)) = pair else {
            return;
        };
        let link = driver
            .connect(output, input)
            .expect("PipeWire link creation should succeed");
        assert!(driver.graph().link(link.id).is_some());
        driver
            .disconnect(link.id)
            .expect("PipeWire link destruction should succeed");
    }
}
