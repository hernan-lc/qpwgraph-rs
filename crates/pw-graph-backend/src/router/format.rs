//! PCM geometry: what a buffer means before anything touches its samples.
//!
//! Every buffer that crosses the router carries interleaved `f32`, and the
//! only thing that distinguishes one from another is its [`AudioFormat`].
//! Reinterpreting a buffer under a different geometry is the classic Windows
//! audio bug — a 44.1 kHz 7.1 endpoint fed 48 kHz stereo does not fail, it
//! plays fast and in the wrong channels — so the router never infers a
//! format: it is declared by the source or sink and converted explicitly.

/// Interleaved sample geometry.
///
/// `channels` is a count, not a mask. Windows channel masks matter when
/// opening an endpoint; once the PCM is inside the router the only thing that
/// survives is "how many interleaved values make one frame".
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AudioFormat {
    pub sample_rate: u32,
    pub channels: u16,
}

impl AudioFormat {
    pub const fn new(sample_rate: u32, channels: u16) -> Self {
        Self {
            sample_rate,
            channels,
        }
    }

    /// Samples in `frames` frames of this format.
    pub const fn samples(&self, frames: usize) -> usize {
        frames * self.channels as usize
    }

    /// Whole frames in a buffer of `samples` values, ignoring a partial frame.
    ///
    /// A partial frame can only arrive from a broken source; dropping it is
    /// safer than shifting every later frame by one channel.
    pub const fn frames(&self, samples: usize) -> usize {
        if self.channels == 0 {
            0
        } else {
            samples / self.channels as usize
        }
    }

    /// Whether this geometry can carry audio at all.
    pub const fn is_valid(&self) -> bool {
        self.sample_rate > 0 && self.channels > 0
    }

    /// Output frames that `frames` input frames become at `target`'s rate,
    /// rounded up, plus one frame of slack for the resampler's fractional
    /// phase.
    ///
    /// Used to size buffers once, on the control thread, so the real-time
    /// path never has to ask whether a conversion will fit.
    pub fn resampled_capacity(&self, target: AudioFormat, frames: usize) -> usize {
        if self.sample_rate == 0 {
            return 0;
        }
        let numerator = frames as u64 * u64::from(target.sample_rate);
        let scaled = numerator.div_ceil(u64::from(self.sample_rate));
        scaled as usize + 1
    }
}

/// How a block of `source` channels becomes a block of `target` channels.
///
/// Deliberately small: the router is a patchbay, not a surround mixer. The
/// rules cover the cases Windows actually produces — mono microphones into
/// stereo routes, stereo routes into mono capture endpoints, and matching
/// pairs — and anything else copies positionally rather than inventing a
/// downmix matrix nobody asked for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelMap {
    /// Same count: a straight copy.
    Identity,
    /// One source channel replicated to every target channel.
    Replicate,
    /// Every source channel averaged into a single target channel.
    Fold,
    /// The first `min(source, target)` channels copied; extra target channels
    /// are silenced and extra source channels are dropped.
    Positional,
}

impl ChannelMap {
    pub fn between(source: u16, target: u16) -> Self {
        match (source, target) {
            (s, t) if s == t => Self::Identity,
            (1, _) => Self::Replicate,
            (_, 1) => Self::Fold,
            _ => Self::Positional,
        }
    }

    /// Remap as many frames as both slices hold, from `src` into `dst`.
    ///
    /// The caller sizes both buffers on the control thread; nothing here
    /// allocates. Returns the number of frames written.
    pub fn apply(
        self,
        src: &[f32],
        src_channels: u16,
        dst: &mut [f32],
        dst_channels: u16,
    ) -> usize {
        let (src_channels, dst_channels) = (src_channels as usize, dst_channels as usize);
        if src_channels == 0 || dst_channels == 0 {
            return 0;
        }
        let frames = (src.len() / src_channels).min(dst.len() / dst_channels);
        match self {
            Self::Identity => {
                dst[..frames * dst_channels].copy_from_slice(&src[..frames * src_channels]);
            }
            Self::Replicate => {
                for frame in 0..frames {
                    let value = src[frame * src_channels];
                    dst[frame * dst_channels..(frame + 1) * dst_channels].fill(value);
                }
            }
            Self::Fold => {
                let scale = 1.0 / src_channels as f32;
                for frame in 0..frames {
                    let block = &src[frame * src_channels..(frame + 1) * src_channels];
                    dst[frame * dst_channels] = block.iter().sum::<f32>() * scale;
                }
            }
            Self::Positional => {
                let shared = src_channels.min(dst_channels);
                for frame in 0..frames {
                    let from = &src[frame * src_channels..];
                    let into = &mut dst[frame * dst_channels..(frame + 1) * dst_channels];
                    into[..shared].copy_from_slice(&from[..shared]);
                    into[shared..].fill(0.0);
                }
            }
        }
        frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_format_converts_between_frames_and_samples() {
        let stereo = AudioFormat::new(48_000, 2);
        assert_eq!(stereo.samples(128), 256);
        assert_eq!(stereo.frames(256), 128);
        // A trailing half frame is dropped rather than shifting the rest of
        // the buffer by one channel.
        assert_eq!(stereo.frames(257), 128);
    }

    #[test]
    fn a_zero_channel_format_is_rejected_rather_than_dividing_by_zero() {
        let broken = AudioFormat::new(48_000, 0);
        assert!(!broken.is_valid());
        assert_eq!(broken.frames(64), 0);
    }

    #[test]
    fn resampled_capacity_rounds_up_and_leaves_room_for_the_fractional_phase() {
        let source = AudioFormat::new(44_100, 2);
        let target = AudioFormat::new(48_000, 2);
        // 480 input frames at 44.1 kHz are 522.4 frames at 48 kHz.
        assert_eq!(source.resampled_capacity(target, 480), 523 + 1);
        // 480 frames at 48 kHz are exactly 441 at 44.1 kHz, and the slack
        // frame is still reserved.
        assert_eq!(target.resampled_capacity(source, 480), 441 + 1);
    }

    #[test]
    fn matching_channel_counts_copy_straight_through() {
        let src = [0.25, -0.5, 0.75, -1.0];
        let mut dst = [0.0; 4];
        assert_eq!(ChannelMap::between(2, 2).apply(&src, 2, &mut dst, 2), 2);
        assert_eq!(dst, src);
    }

    #[test]
    fn a_mono_source_is_replicated_across_every_target_channel() {
        let src = [0.5, -0.25];
        let mut dst = [0.0; 6];
        ChannelMap::between(1, 3).apply(&src, 1, &mut dst, 3);
        assert_eq!(dst, [0.5, 0.5, 0.5, -0.25, -0.25, -0.25]);
    }

    #[test]
    fn a_multichannel_source_folds_into_mono_by_averaging() {
        let src = [1.0, 0.0, -1.0, 1.0];
        let mut dst = [0.0; 2];
        ChannelMap::between(2, 1).apply(&src, 2, &mut dst, 1);
        assert_eq!(dst, [0.5, 0.0]);
    }

    #[test]
    fn mismatched_multichannel_counts_copy_positionally_and_silence_the_rest() {
        let src = [1.0, 2.0, 3.0, 4.0];
        let mut dst = [9.0; 8];
        ChannelMap::between(2, 4).apply(&src, 2, &mut dst, 4);
        // The extra target channels are silenced, not left holding whatever
        // the buffer had from the previous block.
        assert_eq!(dst, [1.0, 2.0, 0.0, 0.0, 3.0, 4.0, 0.0, 0.0]);
    }

    #[test]
    fn remapping_stops_at_whichever_buffer_runs_out_first() {
        let src = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        let mut dst = [0.0; 2];
        assert_eq!(ChannelMap::between(2, 2).apply(&src, 2, &mut dst, 2), 1);
        assert_eq!(dst, [1.0, 2.0]);
    }
}
