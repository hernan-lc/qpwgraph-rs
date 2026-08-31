//! Effects are hosted by whichever child backend supports them.

use super::*;

impl pw_graph_backend::EffectDriver for CompositeDriver {
    fn effect_descriptors(&self) -> Vec<pw_graph_effects::EffectDescriptor> {
        #[cfg(all(target_os = "linux", feature = "pipewire"))]
        {
            self.pipewire
                .as_ref()
                .map(|driver| driver.effect_descriptors())
                .unwrap_or_default()
        }
        #[cfg(not(all(target_os = "linux", feature = "pipewire")))]
        {
            Vec::new()
        }
    }

    fn effect_instances(&self) -> Vec<pw_graph_backend::EffectInstance> {
        #[cfg(all(target_os = "linux", feature = "pipewire"))]
        {
            self.pipewire
                .as_ref()
                .map(|driver| driver.effect_instances())
                .unwrap_or_default()
        }
        #[cfg(not(all(target_os = "linux", feature = "pipewire")))]
        {
            Vec::new()
        }
    }

    fn supports_effect_nodes(&self) -> bool {
        #[cfg(all(target_os = "linux", feature = "pipewire"))]
        {
            self.pipewire
                .as_ref()
                .is_some_and(|driver| driver.supports_effect_nodes())
        }
        #[cfg(not(all(target_os = "linux", feature = "pipewire")))]
        {
            false
        }
    }

    fn create_effect_node(
        &mut self,
        request: pw_graph_backend::EffectNodeRequest,
    ) -> BackendResult<pw_graph_backend::EffectInstance> {
        #[cfg(all(target_os = "linux", feature = "pipewire"))]
        {
            self.mutate_pipewire(|driver| driver.create_effect_node(request))
        }
        #[cfg(not(all(target_os = "linux", feature = "pipewire")))]
        {
            let _ = request;
            Err(Self::unsupported("effect processing is unavailable"))
        }
    }

    fn insert_effect(
        &mut self,
        request: pw_graph_backend::EffectInsertRequest,
    ) -> BackendResult<pw_graph_backend::EffectInstance> {
        #[cfg(all(target_os = "linux", feature = "pipewire"))]
        {
            self.mutate_pipewire(|driver| driver.insert_effect(request))
        }
        #[cfg(not(all(target_os = "linux", feature = "pipewire")))]
        {
            let _ = request;
            Err(Self::unsupported("effect processing is unavailable"))
        }
    }

    fn set_effect_enabled(&mut self, instance_id: &str, enabled: bool) -> BackendResult<()> {
        #[cfg(all(target_os = "linux", feature = "pipewire"))]
        {
            self.pipewire_mut()?
                .set_effect_enabled(instance_id, enabled)
        }
        #[cfg(not(all(target_os = "linux", feature = "pipewire")))]
        {
            let _ = (instance_id, enabled);
            Err(Self::unsupported("effect processing is unavailable"))
        }
    }

    fn set_effect_parameter(
        &mut self,
        instance_id: &str,
        parameter: &str,
        value: f32,
    ) -> BackendResult<()> {
        #[cfg(all(target_os = "linux", feature = "pipewire"))]
        {
            self.pipewire_mut()?
                .set_effect_parameter(instance_id, parameter, value)
        }
        #[cfg(not(all(target_os = "linux", feature = "pipewire")))]
        {
            let _ = (instance_id, parameter, value);
            Err(Self::unsupported("effect processing is unavailable"))
        }
    }

    fn remove_effect(&mut self, instance_id: &str) -> BackendResult<()> {
        #[cfg(all(target_os = "linux", feature = "pipewire"))]
        {
            self.mutate_pipewire(|driver| driver.remove_effect(instance_id))
        }
        #[cfg(not(all(target_os = "linux", feature = "pipewire")))]
        {
            let _ = instance_id;
            Err(Self::unsupported("effect processing is unavailable"))
        }
    }
}
