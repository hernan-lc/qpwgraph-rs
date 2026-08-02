//! Stable, deliberately small ABI shared by WASM effect authors and hosts.
//!
//! WASM modules are compiled for `wasm32-unknown-unknown`. The realtime call
//! has no WASI imports: module discovery, metadata, instantiation, and memory
//! growth happen before the PipeWire callback starts.

use super::{EffectDescriptor, EffectError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const ABI_VERSION: u32 = 1;
pub const EXPORT_METADATA: &str = "effect_metadata";
pub const EXPORT_INIT: &str = "effect_init";
pub const EXPORT_PROCESS: &str = "effect_process";
pub const EXPORT_SET_PARAMETER: &str = "effect_set_parameter";
pub const EXPORT_RESET: &str = "effect_reset";

pub fn required_exports() -> BTreeSet<&'static str> {
    [
        EXPORT_METADATA,
        EXPORT_INIT,
        EXPORT_PROCESS,
        EXPORT_SET_PARAMETER,
        EXPORT_RESET,
    ]
    .into_iter()
    .collect()
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct WasmManifest {
    pub abi_version: u32,
    pub descriptor: EffectDescriptor,
}

impl WasmManifest {
    pub fn from_json(bytes: &[u8]) -> Result<Self, EffectError> {
        let manifest: Self = serde_json::from_slice(bytes)
            .map_err(|error| EffectError::InvalidWasmManifest(error.to_string()))?;
        if manifest.abi_version != ABI_VERSION {
            return Err(EffectError::InvalidWasmManifest(format!(
                "unsupported ABI version {}",
                manifest.abi_version
            )));
        }
        if manifest.descriptor.id.trim().is_empty() {
            return Err(EffectError::InvalidWasmManifest(
                "effect id cannot be empty".into(),
            ));
        }
        Ok(manifest)
    }
}

pub fn validate_exports<'a>(exports: impl IntoIterator<Item = &'a str>) -> Result<(), EffectError> {
    let exports: BTreeSet<&str> = exports.into_iter().collect();
    for required in required_exports() {
        if !exports.contains(required) {
            return Err(EffectError::MissingWasmExport(required.into()));
        }
    }
    Ok(())
}

/// Guest-side ABI shape. A guest SDK can implement these exports without
/// importing WASI. Pointer arguments refer to the module's linear memory.
pub const ABI_DOCUMENTATION: &str =
    "effect_metadata() -> (ptr,len), effect_init(rate,channels,max_frames) -> i32, effect_process(ptr,frames,channels) -> i32, effect_set_parameter(ptr,len,value) -> i32, effect_reset()";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_exports_are_validated() {
        let exports = required_exports();
        validate_exports(exports.iter().copied()).unwrap();
        let missing = [EXPORT_INIT, EXPORT_PROCESS, EXPORT_RESET];
        assert!(validate_exports(missing).is_err());
    }

    #[test]
    fn manifest_rejects_unknown_abi() {
        let descriptor = EffectDescriptor {
            id: "test.effect".into(),
            name: "Test".into(),
            vendor: "test".into(),
            version: "1".into(),
            parameters: Vec::new(),
        };
        let json = serde_json::to_vec(&WasmManifest {
            abi_version: ABI_VERSION + 1,
            descriptor,
        })
        .unwrap();
        assert!(WasmManifest::from_json(&json).is_err());
    }
}
