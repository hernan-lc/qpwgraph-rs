//! Accessors that exist on only one platform: the Windows relay endpoint
//! selection, and the MIDI child Windows polls for device arrival.

use super::*;

impl CompositeDriver {
    #[cfg(target_os = "windows")]
    pub fn has_windows_midi(&self) -> bool {
        self.windows_midi.is_some()
    }

    #[cfg(all(target_os = "windows", feature = "relay"))]
    pub fn windows_relay_endpoint_choices(&self) -> Vec<(String, String)> {
        self.windows_audio
            .as_ref()
            .map(|driver| driver.relay_endpoint_choices())
            .unwrap_or_default()
    }

    #[cfg(all(target_os = "windows", feature = "relay"))]
    pub fn windows_relay_endpoints(&self) -> pw_graph_backend::RelayEndpoints {
        self.windows_audio
            .as_ref()
            .map(|driver| driver.relay_endpoints().clone())
            .unwrap_or_default()
    }

    #[cfg(all(target_os = "windows", feature = "relay"))]
    pub fn set_windows_relay_endpoints(
        &mut self,
        endpoints: pw_graph_backend::RelayEndpoints,
    ) -> BackendResult<()> {
        self.windows_audio
            .as_mut()
            .ok_or_else(|| Self::unsupported("Windows audio backend is unavailable"))?
            .set_relay_endpoints(endpoints)
    }
}
