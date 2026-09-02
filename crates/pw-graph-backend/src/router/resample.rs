//! Rate conversion between two clocks that will never quite agree.
//!
//! Two Windows endpoints nominally at 48 kHz are two separate crystals. Over
//! minutes they drift, and a converter locked to the nominal ratio slowly
//! fills or drains the buffer between them until it dropouts. So the ratio is
//! not a constant here: [`Resampler::set_drift`] nudges it by a few parts per
//! million based on how full the downstream buffer is running, which is the
//! cheap version of what an asynchronous sample-rate converter does.
//!
//! The interpolation itself is linear. That is audibly imperfect for large
//! ratio changes and entirely adequate for the drift corrections and modest
//! 44.1↔48 kHz conversions a patchbay actually performs; a higher-order
//! kernel can replace [`Resampler::process`] without changing its contract.

use super::format::AudioFormat;

/// Largest drift correction applied to the nominal ratio, as a fraction.
///
/// 200 ppm is far beyond real crystal error but still small enough to be
/// inaudible as pitch. Clamping matters: an unclamped correction driven by a
/// buffer-fill error term turns a transient stall into a permanent pitch
/// shift.
const MAX_DRIFT: f64 = 200.0 / 1_000_000.0;

/// What a single [`Resampler::process`] call moved.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Converted {
    /// Input frames the resampler is finished with. The caller must drop
    /// exactly these from the front of its staging buffer.
    pub consumed: usize,
    /// Output frames written.
    pub produced: usize,
}

/// A stateful linear resampler for one interleaved stream.
///
/// The phase and the one-frame history carry across calls, so a stream split
/// into blocks converts identically to the same stream converted whole. That
/// is what makes the router's tests deterministic.
#[derive(Debug)]
pub struct Resampler {
    channels: usize,
    /// Input frames per output frame at the nominal rates.
    nominal_ratio: f64,
    /// `nominal_ratio` after the current drift correction.
    ratio: f64,
    drift: f64,
    /// Fractional read position within the current input block.
    ///
    /// After every call whole frames are consumed until it falls back into
    /// `0.0..1.0`, and the frame it lands on is deliberately *not* consumed:
    /// that frame is the left-hand side of the next interpolation. Keeping it
    /// is what lets a block-split stream convert identically to the same
    /// stream converted whole, with no separate history frame to carry.
    position: f64,
}

impl Resampler {
    pub fn new(source: AudioFormat, target: AudioFormat) -> Self {
        let channels = source.channels.max(1) as usize;
        let nominal_ratio = if target.sample_rate == 0 {
            1.0
        } else {
            f64::from(source.sample_rate) / f64::from(target.sample_rate)
        };
        Self {
            channels,
            nominal_ratio,
            ratio: nominal_ratio,
            drift: 0.0,
            position: 0.0,
        }
    }

    /// Whether the rates match exactly and no drift is being applied, in
    /// which case the caller can copy instead of interpolating.
    pub fn is_passthrough(&self) -> bool {
        self.nominal_ratio == 1.0 && self.drift == 0.0
    }

    /// The ratio actually in use, in input frames per output frame.
    pub fn ratio(&self) -> f64 {
        self.ratio
    }

    /// Current drift correction in parts per million, for diagnostics.
    pub fn drift_ppm(&self) -> f64 {
        self.drift * 1_000_000.0
    }

    /// Nudge the conversion ratio to absorb clock drift.
    ///
    /// Positive `drift` consumes input faster, which drains a buffer that is
    /// filling up. The value is clamped, so a wild error term cannot turn
    /// into an audible pitch shift.
    pub fn set_drift(&mut self, drift: f64) {
        self.drift = drift.clamp(-MAX_DRIFT, MAX_DRIFT);
        self.ratio = self.nominal_ratio * (1.0 + self.drift);
    }

    /// Forget the phase and history.
    ///
    /// Called when a device restarts: interpolating across the gap would
    /// smear the last frame from before the outage into the first frame
    /// after it.
    pub fn reset(&mut self) {
        self.position = 0.0;
    }

    /// Convert from `input` into `output`, both interleaved at `channels`.
    ///
    /// Produces as many output frames as the available input allows, up to
    /// what `output` holds. Nothing here allocates or blocks.
    pub fn process(&mut self, input: &[f32], output: &mut [f32]) -> Converted {
        let channels = self.channels;
        let in_frames = input.len() / channels;
        let max_out = output.len() / channels;
        if in_frames == 0 || max_out == 0 {
            return Converted::default();
        }

        let mut produced = 0;
        while produced < max_out {
            let left = self.position.floor();
            let index = left as usize;
            // The frame after `index` must be present to interpolate towards.
            if index + 1 >= in_frames {
                break;
            }
            let weight = (self.position - left) as f32;
            let base = index * channels;
            let next = base + channels;
            let out_base = produced * channels;
            for channel in 0..channels {
                let before = input[base + channel];
                let after = input[next + channel];
                output[out_base + channel] = before + (after - before) * weight;
            }
            produced += 1;
            self.position += self.ratio;
        }

        // Everything strictly before the read position is finished with. The
        // frame the position sits on stays: it is the left-hand side of the
        // next interpolation, and consuming it would drop one frame at every
        // block boundary.
        let consumed = self.position.floor().min(in_frames as f64) as usize;
        self.position -= consumed as f64;
        Converted { consumed, produced }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STEREO_48: AudioFormat = AudioFormat::new(48_000, 2);

    fn drain(resampler: &mut Resampler, input: &[f32], channels: usize) -> Vec<f32> {
        let mut output = vec![0.0; input.len() * 4 + 64];
        let converted = resampler.process(input, &mut output);
        output.truncate(converted.produced * channels);
        output
    }

    #[test]
    fn matching_rates_are_reported_as_passthrough() {
        let resampler = Resampler::new(STEREO_48, STEREO_48);
        assert!(resampler.is_passthrough());
        assert_eq!(resampler.ratio(), 1.0);
    }

    #[test]
    fn matching_rates_reproduce_the_input_frame_for_frame() {
        let mut resampler = Resampler::new(STEREO_48, STEREO_48);
        let input = [0.0, 1.0, 0.25, 0.75, 0.5, 0.5, 0.75, 0.25];
        let output = drain(&mut resampler, &input, 2);
        // The last frame has nothing to interpolate towards yet, so it is held
        // back rather than duplicated.
        assert_eq!(output, input[..6]);
    }

    #[test]
    fn doubling_the_rate_puts_a_midpoint_between_every_pair_of_frames() {
        let mut resampler =
            Resampler::new(AudioFormat::new(24_000, 1), AudioFormat::new(48_000, 1));
        let output = drain(&mut resampler, &[0.0, 1.0, 0.0], 1);
        assert_eq!(output, vec![0.0, 0.5, 1.0, 0.5]);
    }

    #[test]
    fn halving_the_rate_keeps_every_other_frame() {
        let mut resampler =
            Resampler::new(AudioFormat::new(48_000, 1), AudioFormat::new(24_000, 1));
        let output = drain(&mut resampler, &[0.0, 0.25, 0.5, 0.75, 1.0], 1);
        // The fifth frame is held back: it is the left-hand side of the next
        // interpolation and has nothing yet to interpolate towards.
        assert_eq!(output, vec![0.0, 0.5]);
    }

    #[test]
    fn a_stream_split_into_blocks_converts_the_same_as_the_whole_stream() {
        let whole: Vec<f32> = (0..64).map(|n| n as f32 / 64.0).collect();
        let mut one_shot = Resampler::new(AudioFormat::new(44_100, 1), AudioFormat::new(48_000, 1));
        let expected = drain(&mut one_shot, &whole, 1);

        let mut blocked = Resampler::new(AudioFormat::new(44_100, 1), AudioFormat::new(48_000, 1));
        let mut staging: Vec<f32> = Vec::new();
        let mut actual = Vec::new();
        let mut output = vec![0.0; 256];
        for chunk in whole.chunks(7) {
            staging.extend_from_slice(chunk);
            let converted = blocked.process(&staging, &mut output);
            actual.extend_from_slice(&output[..converted.produced]);
            staging.drain(..converted.consumed);
        }
        // Phase and history carry across calls, so block boundaries are not
        // observable in the result.
        assert_eq!(actual, expected);
    }

    #[test]
    fn drift_is_clamped_so_a_wild_error_term_cannot_shift_pitch() {
        let mut resampler = Resampler::new(STEREO_48, STEREO_48);
        resampler.set_drift(1.0);
        assert!((resampler.drift_ppm() - 200.0).abs() < 1e-9);
        resampler.set_drift(-1.0);
        assert!((resampler.drift_ppm() + 200.0).abs() < 1e-9);
    }

    #[test]
    fn positive_drift_consumes_input_slightly_faster_than_nominal() {
        let mut resampler = Resampler::new(STEREO_48, STEREO_48);
        resampler.set_drift(MAX_DRIFT);
        assert!(resampler.ratio() > 1.0);
        assert!(!resampler.is_passthrough());
    }

    #[test]
    fn resetting_clears_the_phase_so_a_restart_does_not_smear_across_the_gap() {
        let mut resampler =
            Resampler::new(AudioFormat::new(24_000, 1), AudioFormat::new(48_000, 1));
        let before = drain(&mut resampler, &[0.0, 1.0, 0.0], 1);
        resampler.reset();
        let after = drain(&mut resampler, &[0.0, 1.0, 0.0], 1);
        assert_eq!(before, after);
    }

    #[test]
    fn a_short_output_buffer_stops_early_and_reports_what_it_consumed() {
        let mut resampler = Resampler::new(STEREO_48, STEREO_48);
        let input = [1.0, 1.0, 2.0, 2.0, 3.0, 3.0, 4.0, 4.0];
        let mut output = [0.0; 4];
        let converted = resampler.process(&input, &mut output);
        assert_eq!(converted.produced, 2);
        // Only the frames it is finished with are reported, so the caller
        // keeps the rest for the next call rather than dropping audio.
        assert_eq!(converted.consumed, 2);
        assert_eq!(output, [1.0, 1.0, 2.0, 2.0]);
    }
}
