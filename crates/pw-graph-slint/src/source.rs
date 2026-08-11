//! Read-only backend snapshots for the Slint preview.
//!
//! This module deliberately exposes no topology, device, effect, relay, or
//! persistence mutation API. The preview can observe the same graph as the
//! Egui application without becoming a second controller for the session.

use crate::args::Args;
use pw_graph_backend::{DemoDriver, GraphDriver};
use pw_graph_core::Graph;
#[cfg(any(feature = "pipewire", feature = "alsa"))]
use pw_graph_core::GraphError;

#[cfg(feature = "alsa")]
use pw_graph_alsamidi::AlsaMidiDriver;
#[cfg(feature = "pipewire")]
use pw_graph_backend::PipewireDriver;

pub(crate) struct ReadOnlyGraphSource {
    graph: Graph,
    backend_name: String,
    demo: Option<DemoDriver>,
    #[cfg(feature = "pipewire")]
    pipewire: Option<PipewireDriver>,
    #[cfg(feature = "alsa")]
    alsa: Option<AlsaMidiDriver>,
}

impl ReadOnlyGraphSource {
    pub(crate) fn new(args: &Args) -> (Self, String) {
        if args.demo {
            let mut demo = DemoDriver::demo();
            let graph = demo.graph().clone();
            let _ = demo.refresh();
            return (
                Self {
                    graph,
                    backend_name: "demo".into(),
                    demo: Some(demo),
                    #[cfg(feature = "pipewire")]
                    pipewire: None,
                    #[cfg(feature = "alsa")]
                    alsa: None,
                },
                "Slint preview connected to deterministic demo data".into(),
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
            demo: None,
            #[cfg(feature = "pipewire")]
            pipewire,
            #[cfg(feature = "alsa")]
            alsa,
        };
        let status = match source.refresh() {
            Ok(()) if !source.graph.nodes.is_empty() => {
                "Slint preview is observing the live graph (read-only)".into()
            }
            Ok(()) => "No live graph is available; use --demo for a preview graph".into(),
            Err(error) => format!("Could not refresh live graph: {error}"),
        };

        let failures: Vec<_> = [pipewire_error, alsa_error].into_iter().flatten().collect();
        let status = if failures.is_empty() {
            status
        } else {
            format!("{status} · {}", failures.join(" · "))
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
