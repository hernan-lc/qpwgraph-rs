//! Realtime audio effects and the public effect SDK.
//!
//! The processing trait intentionally receives an already allocated,
//! interleaved `f32` buffer. Implementations must not allocate, block, or do
//! filesystem/network work from [`EffectProcessor::process`]. This makes the
//! same API usable from a PipeWire realtime callback and from an offline test.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

pub mod wasm;

pub const NOISE_GATE_ID: &str = "builtin.noise-gate";

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AudioSpec {
    pub sample_rate: u32,
    pub channels: u16,
    pub max_frames: u32,
}

impl AudioSpec {
    pub fn validate(&self) -> Result<(), EffectError> {
        if self.sample_rate == 0 || self.channels == 0 || self.max_frames == 0 {
            return Err(EffectError::InvalidAudioSpec {
                sample_rate: self.sample_rate,
                channels: self.channels,
                max_frames: self.max_frames,
            });
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct EffectParameter {
    pub id: String,
    pub name: String,
    pub minimum: f32,
    pub maximum: f32,
    pub default: f32,
    pub unit: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct EffectDescriptor {
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub version: String,
    pub parameters: Vec<EffectParameter>,
}

#[derive(Clone, Debug, Error, PartialEq)]
pub enum EffectError {
    #[error("invalid audio specification: sample rate {sample_rate}, channels {channels}, max frames {max_frames}")]
    InvalidAudioSpec {
        sample_rate: u32,
        channels: u16,
        max_frames: u32,
    },
    #[error("effect has not been prepared")]
    NotPrepared,
    #[error("audio buffer has {actual} samples, expected {expected}")]
    InvalidBufferLength { actual: usize, expected: usize },
    #[error("audio buffer exceeds the prepared frame limit: {frames} > {max_frames}")]
    FrameLimitExceeded { frames: u32, max_frames: u32 },
    #[error("unsupported effect parameter: {0}")]
    UnsupportedParameter(String),
    #[error("unknown effect: {0}")]
    UnknownEffect(String),
    #[error("invalid parameter value for {id}: {value}")]
    InvalidParameter { id: String, value: f32 },
    #[error("effect module is missing export: {0}")]
    MissingWasmExport(String),
    #[error("invalid effect module manifest: {0}")]
    InvalidWasmManifest(String),
}

/// A processor is created and prepared off the realtime thread.
pub trait EffectProcessor: Send {
    fn descriptor(&self) -> &EffectDescriptor;
    fn prepare(&mut self, spec: AudioSpec) -> Result<(), EffectError>;
    /// Process `frames` of interleaved audio in place.
    ///
    /// Implementations must not allocate, block, panic, or perform I/O here.
    fn process(&mut self, buffer: &mut [f32], frames: u32) -> Result<(), EffectError>;
    fn set_parameter(&mut self, id: &str, value: f32) -> Result<(), EffectError>;
    fn reset(&mut self);
}

pub trait EffectFactory: Send + Sync {
    fn descriptor(&self) -> &EffectDescriptor;
    fn create(&self) -> Box<dyn EffectProcessor>;
}

#[derive(Default)]
pub struct EffectHost {
    factories: BTreeMap<String, Box<dyn EffectFactory>>,
}

impl EffectHost {
    pub fn new() -> Self {
        let mut host = Self::default();
        host.register(Box::new(NoiseGateFactory));
        host
    }

    pub fn register(&mut self, factory: Box<dyn EffectFactory>) {
        let id = factory.descriptor().id.clone();
        self.factories.insert(id, factory);
    }

    pub fn descriptors(&self) -> Vec<EffectDescriptor> {
        self.factories
            .values()
            .map(|factory| factory.descriptor().clone())
            .collect()
    }

    pub fn create(&self, id: &str) -> Result<Box<dyn EffectProcessor>, EffectError> {
        self.factories
            .get(id)
            .map(|factory| factory.create())
            .ok_or_else(|| EffectError::UnknownEffect(id.into()))
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct EffectInstanceConfig {
    pub instance_id: String,
    pub effect_id: String,
    #[serde(default)]
    pub module_path: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub parameters: BTreeMap<String, f32>,
}

fn default_true() -> bool {
    true
}

pub const NOISE_GATE_THRESHOLD: &str = "threshold-db";
pub const NOISE_GATE_ATTACK: &str = "attack-ms";
pub const NOISE_GATE_HOLD: &str = "hold-ms";
pub const NOISE_GATE_RELEASE: &str = "release-ms";
pub const NOISE_GATE_BYPASS: &str = "bypass";

fn noise_gate_descriptor() -> EffectDescriptor {
    EffectDescriptor {
        id: NOISE_GATE_ID.into(),
        name: "Noise Gate".into(),
        vendor: "qpwgraph-rs".into(),
        version: "1.0.0".into(),
        parameters: vec![
            EffectParameter {
                id: NOISE_GATE_THRESHOLD.into(),
                name: "Threshold".into(),
                minimum: -80.0,
                maximum: 0.0,
                default: -45.0,
                unit: "dB".into(),
            },
            EffectParameter {
                id: NOISE_GATE_ATTACK.into(),
                name: "Attack".into(),
                minimum: 0.0,
                maximum: 500.0,
                default: 5.0,
                unit: "ms".into(),
            },
            EffectParameter {
                id: NOISE_GATE_HOLD.into(),
                name: "Hold".into(),
                minimum: 0.0,
                maximum: 2000.0,
                default: 40.0,
                unit: "ms".into(),
            },
            EffectParameter {
                id: NOISE_GATE_RELEASE.into(),
                name: "Release".into(),
                minimum: 0.0,
                maximum: 2000.0,
                default: 120.0,
                unit: "ms".into(),
            },
            EffectParameter {
                id: NOISE_GATE_BYPASS.into(),
                name: "Bypass".into(),
                minimum: 0.0,
                maximum: 1.0,
                default: 0.0,
                unit: "boolean".into(),
            },
        ],
    }
}

struct NoiseGateFactory;

impl EffectFactory for NoiseGateFactory {
    fn descriptor(&self) -> &EffectDescriptor {
        static DESCRIPTOR: std::sync::OnceLock<EffectDescriptor> = std::sync::OnceLock::new();
        DESCRIPTOR.get_or_init(noise_gate_descriptor)
    }

    fn create(&self) -> Box<dyn EffectProcessor> {
        Box::new(NoiseGate::default())
    }
}

#[derive(Clone, Debug)]
pub struct NoiseGate {
    descriptor: EffectDescriptor,
    spec: Option<AudioSpec>,
    threshold_db: f32,
    attack_ms: f32,
    hold_ms: f32,
    release_ms: f32,
    bypass: bool,
    gain: f32,
    hold_frames: u32,
}

impl Default for NoiseGate {
    fn default() -> Self {
        Self {
            descriptor: noise_gate_descriptor(),
            spec: None,
            threshold_db: -45.0,
            attack_ms: 5.0,
            hold_ms: 40.0,
            release_ms: 120.0,
            bypass: false,
            gain: 0.0,
            hold_frames: 0,
        }
    }
}

impl NoiseGate {
    fn parameter(&self, id: &str) -> Option<(f32, f32)> {
        self.descriptor
            .parameters
            .iter()
            .find(|parameter| parameter.id == id)
            .map(|parameter| (parameter.minimum, parameter.maximum))
    }

    fn coefficient(milliseconds: f32, sample_rate: u32) -> f32 {
        if milliseconds <= 0.0 {
            0.0
        } else {
            (-1.0 / (milliseconds * 0.001 * sample_rate as f32)).exp()
        }
    }
}

impl EffectProcessor for NoiseGate {
    fn descriptor(&self) -> &EffectDescriptor {
        &self.descriptor
    }

    fn prepare(&mut self, spec: AudioSpec) -> Result<(), EffectError> {
        spec.validate()?;
        self.spec = Some(spec);
        self.gain = 0.0;
        self.hold_frames = 0;
        Ok(())
    }

    fn process(&mut self, buffer: &mut [f32], frames: u32) -> Result<(), EffectError> {
        let Some(spec) = self.spec.as_ref() else {
            return Err(EffectError::NotPrepared);
        };
        if frames > spec.max_frames {
            return Err(EffectError::FrameLimitExceeded {
                frames,
                max_frames: spec.max_frames,
            });
        }
        let expected = frames as usize * spec.channels as usize;
        if buffer.len() != expected {
            return Err(EffectError::InvalidBufferLength {
                actual: buffer.len(),
                expected,
            });
        }
        if self.bypass {
            return Ok(());
        }

        let threshold = 10.0_f32.powf(self.threshold_db / 20.0);
        let attack = Self::coefficient(self.attack_ms, spec.sample_rate);
        let release = Self::coefficient(self.release_ms, spec.sample_rate);
        let hold_limit = (self.hold_ms * 0.001 * spec.sample_rate as f32) as u32;
        let channels = spec.channels as usize;

        for frame in buffer.chunks_exact_mut(channels) {
            let mut level = 0.0_f32;
            for sample in frame.iter_mut() {
                if !sample.is_finite() {
                    *sample = 0.0;
                }
                level = level.max(sample.abs());
            }

            if level >= threshold {
                self.hold_frames = hold_limit;
                self.gain = 1.0 - (1.0 - self.gain) * attack;
            } else if self.hold_frames > 0 {
                self.hold_frames -= 1;
            } else {
                self.gain *= release;
            }

            for sample in frame {
                *sample *= self.gain;
            }
        }
        Ok(())
    }

    fn set_parameter(&mut self, id: &str, value: f32) -> Result<(), EffectError> {
        let Some((minimum, maximum)) = self.parameter(id) else {
            return Err(EffectError::UnsupportedParameter(id.into()));
        };
        if !value.is_finite() {
            return Err(EffectError::InvalidParameter {
                id: id.into(),
                value,
            });
        }
        let value = value.clamp(minimum, maximum);
        match id {
            NOISE_GATE_THRESHOLD => self.threshold_db = value,
            NOISE_GATE_ATTACK => self.attack_ms = value,
            NOISE_GATE_HOLD => self.hold_ms = value,
            NOISE_GATE_RELEASE => self.release_ms = value,
            NOISE_GATE_BYPASS => self.bypass = value >= 0.5,
            _ => unreachable!("descriptor and parameter match are kept together"),
        }
        Ok(())
    }

    fn reset(&mut self) {
        self.gain = 0.0;
        self.hold_frames = 0;
    }
}

/// A small helper for hosts applying persisted parameters before preparation.
pub fn apply_parameters(
    processor: &mut dyn EffectProcessor,
    parameters: &BTreeMap<String, f32>,
) -> Result<(), EffectError> {
    for (id, value) in parameters {
        processor.set_parameter(id, *value)?;
    }
    Ok(())
}

/// Return the names exported by a module that every host must require.
pub fn required_wasm_exports() -> BTreeSet<&'static str> {
    wasm::required_exports()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prepared_gate() -> NoiseGate {
        let mut gate = NoiseGate::default();
        gate.prepare(AudioSpec {
            sample_rate: 48_000,
            channels: 2,
            max_frames: 128,
        })
        .unwrap();
        gate
    }

    #[test]
    fn silence_is_attenuated() {
        let mut gate = prepared_gate();
        let mut audio = vec![0.001; 64];
        gate.process(&mut audio, 32).unwrap();
        assert!(audio.iter().all(|sample| sample.abs() < 0.001));
    }

    #[test]
    fn loud_signal_opens_gate() {
        let mut gate = prepared_gate();
        gate.set_parameter(NOISE_GATE_ATTACK, 0.0).unwrap();
        let mut audio = vec![0.8; 64];
        gate.process(&mut audio, 32).unwrap();
        assert!(audio.iter().all(|sample| (*sample - 0.8).abs() < 1e-6));
    }

    #[test]
    fn hold_keeps_gate_open_after_signal_drops() {
        let mut gate = prepared_gate();
        gate.set_parameter(NOISE_GATE_ATTACK, 0.0).unwrap();
        gate.set_parameter(NOISE_GATE_HOLD, 100.0).unwrap();
        let mut loud = vec![0.8; 2];
        gate.process(&mut loud, 1).unwrap();
        let mut quiet = vec![0.001; 2];
        gate.process(&mut quiet, 1).unwrap();
        assert!(quiet[0] > 0.0009);
    }

    #[test]
    fn invalid_values_are_safely_sanitized() {
        let mut gate = prepared_gate();
        let mut audio = vec![f32::NAN, f32::INFINITY, -f32::INFINITY, 0.0];
        gate.process(&mut audio, 2).unwrap();
        assert!(audio.iter().all(|sample| sample.is_finite()));
    }

    #[test]
    fn parameters_are_clamped() {
        let mut gate = prepared_gate();
        gate.set_parameter(NOISE_GATE_THRESHOLD, 100.0).unwrap();
        gate.set_parameter(NOISE_GATE_BYPASS, 4.0).unwrap();
        let mut audio = vec![0.5; 2];
        gate.process(&mut audio, 1).unwrap();
        assert_eq!(audio, vec![0.5, 0.5]);
    }
}
