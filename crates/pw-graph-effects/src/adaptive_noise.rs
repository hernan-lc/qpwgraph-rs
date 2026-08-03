//! Adaptive multiband noise suppression.
//!
//! This module deliberately contains all denoiser-specific state and math so
//! the public SDK stays focused on effect registration and host contracts.
//! Its `process` implementation only mutates buffers prepared up front: it
//! performs no allocation, locking, or I/O on PipeWire's realtime thread.

use crate::{
    AudioSpec, EffectDescriptor, EffectError, EffectFactory, EffectParameter, EffectProcessor,
    NOISE_SUPPRESSOR_ADAPTATION, NOISE_SUPPRESSOR_BYPASS, NOISE_SUPPRESSOR_ID,
    NOISE_SUPPRESSOR_REDUCTION, NOISE_SUPPRESSOR_VOICE_PRESERVE,
};

const BANDS: usize = 4;
const CROSSOVERS_HZ: [f32; BANDS - 1] = [180.0, 750.0, 3_200.0];

pub(crate) fn descriptor() -> EffectDescriptor {
    EffectDescriptor {
        id: NOISE_SUPPRESSOR_ID.into(),
        name: "Adaptive Noise Suppressor".into(),
        vendor: "qpwgraph-rs".into(),
        version: "2.0.0".into(),
        parameters: vec![
            EffectParameter {
                id: NOISE_SUPPRESSOR_REDUCTION.into(),
                name: "Reduction".into(),
                minimum: 0.0,
                maximum: 60.0,
                default: 20.0,
                unit: "dB".into(),
            },
            EffectParameter {
                id: NOISE_SUPPRESSOR_ADAPTATION.into(),
                name: "Adaptation".into(),
                minimum: 0.0,
                maximum: 100.0,
                default: 55.0,
                unit: "%".into(),
            },
            EffectParameter {
                id: NOISE_SUPPRESSOR_VOICE_PRESERVE.into(),
                name: "Voice Preserve".into(),
                minimum: 0.0,
                maximum: 100.0,
                default: 70.0,
                unit: "%".into(),
            },
            EffectParameter {
                id: NOISE_SUPPRESSOR_BYPASS.into(),
                name: "Bypass".into(),
                minimum: 0.0,
                maximum: 1.0,
                default: 0.0,
                unit: "boolean".into(),
            },
        ],
    }
}

pub(crate) struct AdaptiveNoiseSuppressorFactory;

impl EffectFactory for AdaptiveNoiseSuppressorFactory {
    fn descriptor(&self) -> &EffectDescriptor {
        static DESCRIPTOR: std::sync::OnceLock<EffectDescriptor> = std::sync::OnceLock::new();
        DESCRIPTOR.get_or_init(descriptor)
    }

    fn create(&self) -> Box<dyn EffectProcessor> {
        Box::new(AdaptiveNoiseSuppressor::default())
    }
}

/// State allocated per stream channel during `prepare`.
#[derive(Clone, Copy)]
struct ChannelState {
    crossover: [f32; BANDS - 1],
    /// Fast and slow estimators are combined to learn stationary noise without
    /// quickly absorbing a word, a click, or a transient into the noise floor.
    noise_fast: [f32; BANDS],
    noise_slow: [f32; BANDS],
    gain: [f32; BANDS],
    foreground_hold: u32,
}

impl Default for ChannelState {
    fn default() -> Self {
        Self {
            crossover: [0.0; BANDS - 1],
            noise_fast: [0.002; BANDS],
            noise_slow: [0.002; BANDS],
            gain: [1.0; BANDS],
            foreground_hold: 0,
        }
    }
}

/// A four-band adaptive Wiener suppressor with dual-rate noise tracking and
/// foreground hold. It is deliberately more transparent than a noise gate:
/// speech and transients open only the bands that contain foreground energy.
pub struct AdaptiveNoiseSuppressor {
    descriptor: EffectDescriptor,
    spec: Option<AudioSpec>,
    reduction_db: f32,
    adaptation: f32,
    voice_preserve: f32,
    bypass: bool,
    channels: Vec<ChannelState>,
}

impl Default for AdaptiveNoiseSuppressor {
    fn default() -> Self {
        Self {
            descriptor: descriptor(),
            spec: None,
            reduction_db: 20.0,
            adaptation: 55.0,
            voice_preserve: 70.0,
            bypass: false,
            channels: Vec::new(),
        }
    }
}

impl AdaptiveNoiseSuppressor {
    fn parameter(&self, id: &str) -> Option<(f32, f32)> {
        self.descriptor
            .parameters
            .iter()
            .find(|parameter| parameter.id == id)
            .map(|parameter| (parameter.minimum, parameter.maximum))
    }

    fn coefficient(milliseconds: f32, sample_rate: u32) -> f32 {
        (-1.0 / (milliseconds.max(0.1) * 0.001 * sample_rate as f32)).exp()
    }

    fn noise_floor(
        state: &mut ChannelState,
        index: usize,
        level: f32,
        rate: f32,
        foreground: bool,
    ) -> f32 {
        let fast_rate = if foreground { rate * 0.005 } else { rate };
        let slow_rate = if foreground {
            rate * 0.0002
        } else {
            rate * 0.12
        };
        state.noise_fast[index] += (level - state.noise_fast[index]) * fast_rate;
        state.noise_slow[index] += (level - state.noise_slow[index]) * slow_rate;
        state.noise_fast[index]
            .min(state.noise_slow[index])
            .max(1e-7)
    }

    fn split_bands(
        input: f32,
        state: &mut ChannelState,
        coefficients: [f32; BANDS - 1],
    ) -> [f32; BANDS] {
        let mut previous = 0.0;
        let mut bands = [0.0; BANDS];
        for index in 0..BANDS - 1 {
            state.crossover[index] += coefficients[index] * (input - state.crossover[index]);
            bands[index] = state.crossover[index] - previous;
            previous = state.crossover[index];
        }
        bands[BANDS - 1] = input - previous;
        bands
    }
}

impl EffectProcessor for AdaptiveNoiseSuppressor {
    fn descriptor(&self) -> &EffectDescriptor {
        &self.descriptor
    }

    fn prepare(&mut self, spec: AudioSpec) -> Result<(), EffectError> {
        spec.validate()?;
        self.channels = vec![ChannelState::default(); spec.channels as usize];
        self.spec = Some(spec);
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
        let channels = spec.channels as usize;
        let expected = frames as usize * channels;
        if buffer.len() != expected {
            return Err(EffectError::InvalidBufferLength {
                actual: buffer.len(),
                expected,
            });
        }
        if self.bypass {
            return Ok(());
        }

        let sample_rate = spec.sample_rate;
        let crossovers = CROSSOVERS_HZ.map(|frequency| {
            1.0 - (-2.0 * std::f32::consts::PI * frequency / sample_rate as f32).exp()
        });
        let adaptation_ms = 2_200.0 - self.adaptation * 20.0;
        let noise_rate = 1.0 - Self::coefficient(adaptation_ms, sample_rate);
        let open_coefficient = Self::coefficient(8.0, sample_rate);
        let close_coefficient = Self::coefficient(150.0, sample_rate);
        let hold_frames = (0.035 * sample_rate as f32) as u32;
        let minimum_gain = 10.0_f32.powf(-self.reduction_db / 20.0);
        let voice_protection = self.voice_preserve / 100.0;

        for frame in buffer.chunks_exact_mut(channels) {
            for (sample, state) in frame.iter_mut().zip(&mut self.channels) {
                let input = if sample.is_finite() { *sample } else { 0.0 };
                let bands = Self::split_bands(input, state, crossovers);
                let foreground = bands.iter().enumerate().any(|(index, band)| {
                    // Mid bands receive a lower threshold because most speech
                    // energy is concentrated there.
                    let multiplier = if index == 1 || index == 2 { 2.0 } else { 2.8 };
                    band.abs() > state.noise_fast[index] * multiplier
                });
                if foreground {
                    state.foreground_hold = hold_frames;
                } else {
                    state.foreground_hold = state.foreground_hold.saturating_sub(1);
                }
                let protected = state.foreground_hold > 0;
                let mut output = 0.0;

                for (index, band) in bands.into_iter().enumerate() {
                    let floor = Self::noise_floor(state, index, band.abs(), noise_rate, protected);
                    let noise_power = floor * floor;
                    let signal_power = (band * band - noise_power).max(0.0);
                    let wiener = signal_power / (signal_power + noise_power + 1e-12);
                    let preserve = if protected && (index == 1 || index == 2) {
                        0.15 + 0.7 * voice_protection
                    } else {
                        0.0
                    };
                    let target = (minimum_gain + (1.0 - minimum_gain) * wiener).max(preserve);
                    let coefficient = if target > state.gain[index] {
                        open_coefficient
                    } else {
                        close_coefficient
                    };
                    state.gain[index] = target + (state.gain[index] - target) * coefficient;
                    output += band * state.gain[index];
                }
                *sample = output;
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
        match id {
            NOISE_SUPPRESSOR_REDUCTION => self.reduction_db = value.clamp(minimum, maximum),
            NOISE_SUPPRESSOR_ADAPTATION => self.adaptation = value.clamp(minimum, maximum),
            NOISE_SUPPRESSOR_VOICE_PRESERVE => self.voice_preserve = value.clamp(minimum, maximum),
            NOISE_SUPPRESSOR_BYPASS => self.bypass = value.clamp(minimum, maximum) >= 0.5,
            _ => unreachable!("descriptor and parameter match are kept together"),
        }
        Ok(())
    }

    fn reset(&mut self) {
        self.channels.fill(ChannelState::default());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_bank_reconstructs_the_input() {
        let mut state = ChannelState::default();
        let bands = AdaptiveNoiseSuppressor::split_bands(0.42, &mut state, [0.1, 0.2, 0.3]);
        assert!((bands.iter().sum::<f32>() - 0.42).abs() < 1e-6);
    }

    #[test]
    fn descriptor_exposes_voice_preservation() {
        assert!(descriptor()
            .parameters
            .iter()
            .any(|parameter| parameter.id == NOISE_SUPPRESSOR_VOICE_PRESERVE));
    }
}
