//! Sample-rate and channel conversion between a session's negotiated audio
//! format and the engine's local format.
//!
//! # Why this exists
//!
//! Every session negotiates its own geometry: 16, 24 or 48 kHz, mono or
//! stereo. The application on this end has exactly one geometry — whatever
//! its capture and playback endpoints run at. Without a conversion step in
//! between, decoded samples from a 16 kHz mono phone would be handed to a
//! 48 kHz stereo endpoint as if they were already in its format, which plays
//! back at three times the pitch through one channel; and two peers with
//! different formats would interleave into each other.
//!
//! So each session owns a converter in each direction, and the engine mixes
//! only after everything has been brought into one common format.
//!
//! Interpolation is linear. The supported rate pairs are all small integer
//! ratios (2:1, 3:1, 3:2), the audio is speech-band, and a polyphase filter
//! bank would cost latency on a path whose whole purpose is to stay under a
//! human's round-trip perception threshold.

/// Interleaved-PCM converter between two geometries. One instance is stateful
/// and belongs to exactly one direction of one session: it carries the last
/// input frame across calls so the interpolation does not restart (and click)
/// at every buffer boundary.
pub struct Converter {
    in_rate: u32,
    out_rate: u32,
    in_channels: usize,
    out_channels: usize,
    /// Fractional read position carried between calls, in input frames.
    position: f64,
    /// Final input frame of the previous call, per output channel.
    previous: Vec<f32>,
    have_previous: bool,
    /// Channel-mapped input, reused so the realtime path does not allocate
    /// after the first call.
    mapped: Vec<f32>,
}

impl Converter {
    pub fn new(in_rate: u32, in_channels: u16, out_rate: u32, out_channels: u16) -> Self {
        let out_channels = out_channels.max(1) as usize;
        Self {
            in_rate: in_rate.max(1),
            out_rate: out_rate.max(1),
            in_channels: in_channels.max(1) as usize,
            out_channels,
            position: 0.0,
            previous: vec![0.0; out_channels],
            have_previous: false,
            mapped: Vec::new(),
        }
    }

    /// Like [`Self::new`], but with the internal buffer already grown for an
    /// input of `max_input_samples`.
    ///
    /// Use this whenever the converter will be reachable from a realtime
    /// callback. `new` alone leaves `mapped` empty, so the first conversion —
    /// and any conversion of a larger quantum than has been seen before —
    /// allocates, on the one thread that must not.
    pub fn with_capacity(
        in_rate: u32,
        in_channels: u16,
        out_rate: u32,
        out_channels: u16,
        max_input_samples: usize,
    ) -> Self {
        let mut converter = Self::new(in_rate, in_channels, out_rate, out_channels);
        converter.prepare(max_input_samples);
        converter
    }

    /// Grow the internal buffer so [`Self::convert`] of up to
    /// `max_input_samples` interleaved input samples cannot reallocate.
    ///
    /// Call it from setup code only — it allocates by design.
    pub fn prepare(&mut self, max_input_samples: usize) {
        let needed = self.mapped_capacity_for(max_input_samples);
        if self.mapped.capacity() < needed {
            let extra = needed - self.mapped.len();
            self.mapped.reserve(extra);
        }
    }

    /// Interleaved samples `mapped` must hold for an input of
    /// `max_input_samples`: the input's frame count times the *output* channel
    /// count, since channel mapping happens before resampling.
    fn mapped_capacity_for(&self, max_input_samples: usize) -> usize {
        (max_input_samples / self.in_channels + 1) * self.out_channels
    }

    /// Interleaved samples [`Self::convert`] can emit for an input of
    /// `max_input_samples`, so callers can size the destination `Vec`.
    ///
    /// The resampler emits at most one frame per `out_rate / in_rate` step
    /// plus the carried fractional position, so a whole extra frame of
    /// headroom covers the boundary case.
    pub fn output_capacity_for(&self, max_input_samples: usize) -> usize {
        let in_frames = max_input_samples / self.in_channels + 1;
        let out_frames =
            (in_frames as u64 * self.out_rate as u64).div_ceil(self.in_rate as u64) as usize + 1;
        out_frames * self.out_channels
    }

    /// True when input and output geometries match, so `convert` is a copy.
    pub fn is_identity(&self) -> bool {
        self.in_rate == self.out_rate && self.in_channels == self.out_channels
    }

    /// Convert `input` (interleaved, input geometry) into `out` (interleaved,
    /// output geometry). `out` is cleared first and grown as needed.
    pub fn convert(&mut self, input: &[f32], out: &mut Vec<f32>) {
        out.clear();
        if input.is_empty() {
            return;
        }
        if self.is_identity() {
            out.extend_from_slice(input);
            return;
        }
        self.map_channels(input, true);
        self.resample(out, true);
    }

    /// Convert using only storage that was prepared by [`Self::with_capacity`]
    /// (or [`Self::prepare`]). Returns `false` instead of growing either
    /// buffer when a caller hands over a larger-than-prepared quantum.
    ///
    /// The PipeWire callback uses this entry point. Keeping the growth-capable
    /// [`Self::convert`] API for ordinary callers is useful, but it must not
    /// be reachable from a realtime path because even a no-op `reserve` is an
    /// unnecessary allocation-policy escape hatch there.
    pub fn try_convert_prepared(&mut self, input: &[f32], out: &mut Vec<f32>) -> bool {
        out.clear();
        if input.is_empty() {
            return true;
        }
        let output_capacity = self.output_capacity_for(input.len());
        if out.capacity() < output_capacity {
            return false;
        }
        if self.is_identity() {
            out.extend_from_slice(input);
            return true;
        }
        let mapped_capacity = self.mapped_capacity_for(input.len());
        if self.mapped.capacity() < mapped_capacity {
            return false;
        }
        self.map_channels(input, false);
        self.resample(out, false);
        true
    }

    /// Fold or spread the input's channels into the output channel count.
    /// Only 1 and 2 are negotiable, so this is downmix, duplicate, or copy.
    fn map_channels(&mut self, input: &[f32], allow_growth: bool) {
        let frames = input.len() / self.in_channels;
        self.mapped.clear();
        if allow_growth {
            self.mapped.reserve(frames * self.out_channels);
        } else {
            debug_assert!(
                self.mapped.capacity() >= frames * self.out_channels,
                "prepared converter received an oversized quantum"
            );
        }
        if self.in_channels == self.out_channels {
            self.mapped
                .extend_from_slice(&input[..frames * self.in_channels]);
            return;
        }
        for frame in 0..frames {
            let source = &input[frame * self.in_channels..(frame + 1) * self.in_channels];
            if self.out_channels == 1 {
                let sum: f32 = source.iter().sum();
                self.mapped.push(sum / self.in_channels as f32);
            } else {
                // Spread whatever we have across the output channels; with the
                // negotiable set this is a mono source duplicated to stereo.
                for channel in 0..self.out_channels {
                    self.mapped.push(source[channel.min(source.len() - 1)]);
                }
            }
        }
    }

    fn resample(&mut self, out: &mut Vec<f32>, allow_growth: bool) {
        let channels = self.out_channels;
        let frames = self.mapped.len() / channels;
        if frames == 0 {
            return;
        }
        if self.in_rate == self.out_rate {
            if !allow_growth {
                debug_assert!(out.len() + self.mapped.len() <= out.capacity());
            }
            out.extend_from_slice(&self.mapped[..frames * channels]);
            self.remember_last(frames);
            return;
        }
        let step = self.in_rate as f64 / self.out_rate as f64;
        // Sample -1 is the previous call's final frame; before the very first
        // call there is nothing to interpolate from, so start at frame 0.
        let mut position = if self.have_previous {
            self.position
        } else {
            self.position.max(0.0)
        };
        let limit = (frames - 1) as f64;
        while position <= limit {
            if !allow_growth {
                debug_assert!(out.len() + channels <= out.capacity());
            }
            let base = position.floor();
            let fraction = (position - base) as f32;
            let index = base as isize;
            for channel in 0..channels {
                let left = self.sample_at(index, channel);
                let right = self.sample_at(index + 1, channel);
                out.push(left + (right - left) * fraction);
            }
            position += step;
        }
        // Carry the leftover fraction into the next call, measured from the
        // start of the next input buffer.
        self.position = position - frames as f64;
        self.remember_last(frames);
    }

    fn remember_last(&mut self, frames: usize) {
        let channels = self.out_channels;
        let last = (frames - 1) * channels;
        self.previous.clear();
        self.previous
            .extend_from_slice(&self.mapped[last..last + channels]);
        self.have_previous = true;
    }

    fn sample_at(&self, index: isize, channel: usize) -> f32 {
        let channels = self.out_channels;
        if index < 0 {
            return if self.have_previous {
                self.previous[channel]
            } else {
                self.mapped[channel]
            };
        }
        let index = index as usize;
        let frames = self.mapped.len() / channels;
        // Reading one past the end happens only on the final interpolation
        // step; holding the last sample is the right boundary behaviour.
        let index = index.min(frames.saturating_sub(1));
        self.mapped[index * channels + channel]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching_geometry_is_a_straight_copy() {
        let mut converter = Converter::new(48_000, 2, 48_000, 2);
        assert!(converter.is_identity());
        let mut out = Vec::new();
        converter.convert(&[1.0, 2.0, 3.0, 4.0], &mut out);
        assert_eq!(out, vec![1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn mono_spreads_to_stereo_and_stereo_folds_to_mono() {
        let mut up = Converter::new(48_000, 1, 48_000, 2);
        let mut out = Vec::new();
        up.convert(&[1.0, -1.0], &mut out);
        assert_eq!(out, vec![1.0, 1.0, -1.0, -1.0]);

        let mut down = Converter::new(48_000, 2, 48_000, 1);
        down.convert(&[1.0, 0.0, 0.5, 0.5], &mut out);
        assert_eq!(out, vec![0.5, 0.5]);
    }

    #[test]
    fn upsampling_produces_the_expected_rate_ratio() {
        // 16 kHz to 48 kHz is the case that used to play back three times too
        // fast, because nothing converted at all.
        let mut converter = Converter::new(16_000, 1, 48_000, 1);
        let input: Vec<f32> = (0..160).map(|i| i as f32).collect();
        let mut out = Vec::new();
        converter.convert(&input, &mut out);
        // The very first buffer is a couple of samples short: there is no
        // previous frame to interpolate the tail against yet. The carried
        // position makes that up on the next call, which is what
        // `total_output_length_tracks_the_ratio_over_many_buffers` pins down.
        assert!(
            (out.len() as i64 - 480).abs() <= 4,
            "10 ms at 16 kHz should become ~480 samples at 48 kHz, got {}",
            out.len()
        );
    }

    #[test]
    fn downsampling_produces_the_expected_rate_ratio() {
        let mut converter = Converter::new(48_000, 1, 24_000, 1);
        let input: Vec<f32> = (0..480).map(|i| i as f32).collect();
        let mut out = Vec::new();
        converter.convert(&input, &mut out);
        assert!((out.len() as i64 - 240).abs() <= 2, "got {}", out.len());
    }

    #[test]
    fn a_ramp_stays_a_ramp_across_buffer_boundaries() {
        // Continuity across calls is the point of the carried state: a
        // per-buffer reset would put a step discontinuity — an audible click —
        // at every frame boundary.
        let mut converter = Converter::new(24_000, 1, 48_000, 1);
        let mut produced = Vec::new();
        let mut out = Vec::new();
        for block in 0..4 {
            let input: Vec<f32> = (0..240).map(|i| (block * 240 + i) as f32).collect();
            converter.convert(&input, &mut out);
            produced.extend_from_slice(&out);
        }
        // Every step of the output ramp should be about half an input step.
        for pair in produced.windows(2) {
            let delta = pair[1] - pair[0];
            assert!(
                (delta - 0.5).abs() < 0.01,
                "discontinuity of {delta} in the resampled ramp"
            );
        }
    }

    #[test]
    fn total_output_length_tracks_the_ratio_over_many_buffers() {
        // The fractional position must carry, or the stream slowly drifts.
        let mut converter = Converter::new(16_000, 1, 48_000, 1);
        let mut total = 0usize;
        let mut out = Vec::new();
        for _ in 0..100 {
            converter.convert(&vec![0.0; 160], &mut out);
            total += out.len();
        }
        assert!(
            (total as i64 - 48_000).abs() < 10,
            "100 buffers of 10 ms should be ~48000 samples, got {total}"
        );
    }

    /// Run `converter` over `blocks` inputs of `input_samples` and assert that
    /// neither the internal nor the destination buffer ever reallocates.
    /// Capacity growth is the observable proxy for a heap allocation on a
    /// path where allocating is the bug.
    fn assert_no_growth(
        converter: &mut Converter,
        out: &mut Vec<f32>,
        input_samples: usize,
        blocks: usize,
    ) {
        let mapped_capacity = converter.mapped.capacity();
        let previous_capacity = converter.previous.capacity();
        let out_capacity = out.capacity();
        let input = vec![0.25f32; input_samples];
        for block in 0..blocks {
            converter.convert(&input, out);
            assert_eq!(
                converter.mapped.capacity(),
                mapped_capacity,
                "internal buffer grew on block {block}"
            );
            assert_eq!(
                converter.previous.capacity(),
                previous_capacity,
                "carried-frame buffer grew on block {block}"
            );
            assert_eq!(
                out.capacity(),
                out_capacity,
                "output buffer grew on block {block}"
            );
        }
    }

    #[test]
    fn prepared_buffers_do_not_grow_for_rate_expansion() {
        // 16 -> 48 kHz is the largest supported ratio, so it is the case that
        // would allocate first if the sizing arithmetic were short.
        let input_samples = 160;
        let mut converter = Converter::with_capacity(16_000, 1, 48_000, 1, input_samples);
        let mut out = Vec::with_capacity(converter.output_capacity_for(input_samples));
        assert_no_growth(&mut converter, &mut out, input_samples, 50);
    }

    #[test]
    fn prepared_buffers_do_not_grow_for_channel_expansion() {
        let input_samples = 480;
        let mut converter = Converter::with_capacity(48_000, 1, 48_000, 2, input_samples);
        let mut out = Vec::with_capacity(converter.output_capacity_for(input_samples));
        assert_no_growth(&mut converter, &mut out, input_samples, 50);
    }

    #[test]
    fn prepared_buffers_do_not_grow_for_combined_expansion() {
        // Mono 16 kHz in, stereo 48 kHz out: six output samples per input one.
        let input_samples = 160;
        let mut converter = Converter::with_capacity(16_000, 1, 48_000, 2, input_samples);
        let mut out = Vec::with_capacity(converter.output_capacity_for(input_samples));
        assert_no_growth(&mut converter, &mut out, input_samples, 50);
    }

    #[test]
    fn prepared_buffers_cover_every_negotiable_geometry_pair() {
        // The session-setup path sizes from one global maximum quantum; this
        // pins that the bound really does hold for every pair it can be
        // asked to serve, including the fractional 48 -> 24 and 24 -> 48 ones.
        let quantum = crate::MAX_REALTIME_QUANTUM_SAMPLES;
        for in_rate in crate::SAMPLE_RATES_HZ {
            for out_rate in crate::SAMPLE_RATES_HZ {
                for in_channels in [1u16, 2] {
                    for out_channels in [1u16, 2] {
                        let mut converter = Converter::with_capacity(
                            in_rate,
                            in_channels,
                            out_rate,
                            out_channels,
                            quantum,
                        );
                        if converter.is_identity() {
                            continue;
                        }
                        let mut out = Vec::with_capacity(converter.output_capacity_for(quantum));
                        // Also exercise short blocks: the carried fractional
                        // position is what makes an occasional block emit one
                        // frame more than the nominal ratio.
                        for samples in [quantum, quantum - in_channels as usize, quantum] {
                            assert_no_growth(&mut converter, &mut out, samples, 8);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn preparation_does_not_change_conversion_results() {
        let input: Vec<f32> = (0..160).map(|i| i as f32).collect();
        let mut plain = Converter::new(16_000, 1, 48_000, 2);
        let mut prepared = Converter::with_capacity(16_000, 1, 48_000, 2, 160);
        let (mut a, mut b) = (Vec::new(), Vec::new());
        for _ in 0..10 {
            plain.convert(&input, &mut a);
            prepared.convert(&input, &mut b);
            assert_eq!(a, b);
        }
    }
}
