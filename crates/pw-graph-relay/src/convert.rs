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
        self.map_channels(input);
        self.resample(out);
    }

    /// Fold or spread the input's channels into the output channel count.
    /// Only 1 and 2 are negotiable, so this is downmix, duplicate, or copy.
    fn map_channels(&mut self, input: &[f32]) {
        let frames = input.len() / self.in_channels;
        self.mapped.clear();
        self.mapped.reserve(frames * self.out_channels);
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

    fn resample(&mut self, out: &mut Vec<f32>) {
        let channels = self.out_channels;
        let frames = self.mapped.len() / channels;
        if frames == 0 {
            return;
        }
        if self.in_rate == self.out_rate {
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
}
