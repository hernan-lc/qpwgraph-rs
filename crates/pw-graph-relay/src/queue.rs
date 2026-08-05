//! PCM queues bridging realtime audio producers/consumers and the relay
//! engine's network worker threads.
//!
//! The PipeWire side may call `try_push`/`try_pull` from a realtime callback,
//! so those paths use `try_lock` and silently skip a quantum when the queue is
//! busy — the same conservative discipline `pipewire/effects.rs` applies to
//! processor state. Network workers use the blocking variants because a short
//! mutex hold on this side never risks an xrun.
//!
//! # Latency discipline
//!
//! Two properties keep the queue from becoming the dominant source of relay
//! delay:
//!
//! - **A target depth, not just a capacity.** Capacity alone is a disaster
//!   bound: once a consumer stalls, a drop-oldest queue fills to capacity and
//!   *stays* there, so every later sample inherits the full backlog forever.
//!   [`PcmQueue::set_target_depth`] trims to a low watermark on every push,
//!   so a single stall costs a glitch instead of permanent added latency.
//! - **Blocking consumers wake immediately.** [`PcmQueue::pop_exact_timeout`]
//!   parks on a condvar rather than polling, so the network sender transmits
//!   a frame the moment it is complete instead of up to a poll interval
//!   later.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

/// Hard bound: about 340 ms of stereo 48 kHz audio in f32 samples. This is a
/// backstop for a wedged consumer, not the working depth — see
/// [`PcmQueue::set_target_depth`].
pub const DEFAULT_QUEUE_CAPACITY: usize = 32 * 1024;

/// Frames of headroom on the capture-to-network path.
///
/// The floor is set by the graph quantum, not by the codec: PipeWire commonly
/// runs 1024 samples (about 21 ms), so one callback delivers roughly two
/// 10 ms frames in a single push. A cap of two would therefore sit at its
/// limit after every quantum and discard the previous one. Four frames leave
/// a full quantum of slack — still around 40 ms, against the 1.3 s a
/// capacity-only queue used to accumulate.
pub const CAPTURE_DEPTH_FRAMES: usize = 4;

/// Frames of headroom on the network-to-playback path. The same quantum
/// argument applies, plus the receiver decodes in bursts whenever the jitter
/// buffer releases several frames at once. Too tight a cap here discards
/// audio that arrived perfectly well.
pub const PLAYBACK_DEPTH_FRAMES: usize = 4;

/// A bounded, drop-oldest FIFO of interleaved f32 samples.
pub struct PcmQueue {
    inner: Mutex<VecDeque<f32>>,
    /// Signalled after every push so a blocked consumer wakes at once.
    ready: Condvar,
    capacity: usize,
    /// Working depth in samples; 0 means "capacity only".
    target_depth: AtomicUsize,
}

impl PcmQueue {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(VecDeque::with_capacity(capacity)),
            ready: Condvar::new(),
            capacity,
            target_depth: AtomicUsize::new(0),
        }
    }

    /// Set the working depth in samples. Pushes trim the oldest audio down to
    /// this many samples, bounding the queue's contribution to end-to-end
    /// latency regardless of how far a consumer once fell behind. A value of
    /// 0 restores plain capacity-bounded behaviour.
    pub fn set_target_depth(&self, samples: usize) {
        self.target_depth
            .store(samples.min(self.capacity), Ordering::Relaxed);
    }

    pub fn target_depth(&self) -> usize {
        self.target_depth.load(Ordering::Relaxed)
    }

    /// The effective push limit: the target depth when one is set, otherwise
    /// the hard capacity.
    fn limit(&self) -> usize {
        match self.target_depth.load(Ordering::Relaxed) {
            0 => self.capacity,
            depth => depth,
        }
    }

    /// Append samples, dropping the oldest samples once the queue exceeds its
    /// working depth. Fresh audio wins: relay latency matters more than
    /// completeness when a consumer stalls.
    pub fn push(&self, samples: &[f32]) {
        let limit = self.limit();
        {
            let Ok(mut queue) = self.inner.lock() else {
                return;
            };
            push_locked(&mut queue, samples, limit);
        }
        self.ready.notify_one();
    }

    /// Realtime-safe variant: never blocks. Returns `false` when the lock was
    /// busy and this quantum had to be skipped.
    ///
    /// The `notify_one` after the push is a futex wake only when a consumer is
    /// actually parked, and it happens with the lock already released; that
    /// bounded syscall is what lets the sender transmit without a polling
    /// delay, and it cannot block the caller.
    pub fn try_push(&self, samples: &[f32]) -> bool {
        let limit = self.limit();
        {
            let Ok(mut queue) = self.inner.try_lock() else {
                return false;
            };
            push_locked(&mut queue, samples, limit);
        }
        self.ready.notify_one();
        true
    }

    /// Copy up to `out.len()` samples into `out`, returning how many were
    /// available. Remaining entries of `out` are left untouched.
    pub fn pull(&self, out: &mut [f32]) -> usize {
        let Ok(mut queue) = self.inner.lock() else {
            return 0;
        };
        drain_into(&mut queue, out)
    }

    /// Realtime-safe variant of [`Self::pull`]. Returns 0 when the lock is busy.
    pub fn try_pull(&self, out: &mut [f32]) -> usize {
        let Ok(mut queue) = self.inner.try_lock() else {
            return 0;
        };
        drain_into(&mut queue, out)
    }

    /// Take exactly `count` samples, or nothing at all. Senders use this to
    /// keep codec frame sizes exact.
    pub fn pop_exact(&self, count: usize) -> Option<Vec<f32>> {
        let mut queue = self.inner.lock().ok()?;
        if queue.len() < count {
            return None;
        }
        Some(queue.drain(..count).collect())
    }

    /// Blocking [`Self::pop_exact`]: park until `count` samples are available
    /// or `timeout` elapses. The sender uses this so a completed frame goes
    /// on the wire immediately instead of waiting out a poll interval, while
    /// the timeout still lets it notice a session teardown.
    pub fn pop_exact_timeout(&self, count: usize, timeout: Duration) -> Option<Vec<f32>> {
        let queue = self.inner.lock().ok()?;
        let (mut queue, _) = self
            .ready
            .wait_timeout_while(queue, timeout, |queue| queue.len() < count)
            .ok()?;
        if queue.len() < count {
            return None;
        }
        Some(queue.drain(..count).collect())
    }

    pub fn len(&self) -> usize {
        self.inner.lock().map(|queue| queue.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn clear(&self) {
        if let Ok(mut queue) = self.inner.lock() {
            queue.clear();
        }
    }
}

fn drain_into(queue: &mut VecDeque<f32>, out: &mut [f32]) -> usize {
    let count = out.len().min(queue.len());
    for (slot, sample) in out.iter_mut().take(count).zip(queue.drain(..count)) {
        *slot = sample;
    }
    count
}

fn push_locked(queue: &mut VecDeque<f32>, samples: &[f32], limit: usize) {
    if samples.len() >= limit {
        // An oversized write keeps only its tail; history would be stale.
        queue.clear();
        queue.extend(samples.iter().copied().skip(samples.len() - limit));
        return;
    }
    queue.extend(samples.iter().copied());
    while queue.len() > limit {
        queue.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn push_and_pull_round_trip() {
        let queue = PcmQueue::new(64);
        queue.push(&[1.0, 2.0, 3.0]);
        assert_eq!(queue.len(), 3);
        let mut out = [0.0; 4];
        assert_eq!(queue.pull(&mut out), 3);
        assert_eq!(out[..3], [1.0, 2.0, 3.0]);
        assert!(queue.is_empty());
    }

    #[test]
    fn overflow_drops_oldest_samples() {
        let queue = PcmQueue::new(4);
        queue.push(&[1.0, 2.0, 3.0]);
        queue.push(&[4.0, 5.0, 6.0]);
        let mut out = [0.0; 4];
        assert_eq!(queue.pull(&mut out), 4);
        assert_eq!(out, [3.0, 4.0, 5.0, 6.0]);
    }

    #[test]
    fn oversized_write_keeps_tail() {
        let queue = PcmQueue::new(2);
        queue.push(&[1.0, 2.0, 3.0, 4.0]);
        let mut out = [0.0; 2];
        assert_eq!(queue.pull(&mut out), 2);
        assert_eq!(out, [3.0, 4.0]);
    }

    #[test]
    fn pop_exact_requires_full_frame() {
        let queue = PcmQueue::new(16);
        queue.push(&[1.0, 2.0]);
        assert!(queue.pop_exact(4).is_none());
        queue.push(&[3.0, 4.0]);
        assert_eq!(queue.pop_exact(4), Some(vec![1.0, 2.0, 3.0, 4.0]));
        assert!(queue.is_empty());
    }

    #[test]
    fn target_depth_bounds_standing_latency() {
        // Capacity would allow a full second of backlog; the target depth is
        // what actually decides how stale the oldest sample can be.
        let queue = PcmQueue::new(1024);
        queue.set_target_depth(4);
        for value in 0..100 {
            queue.push(&[value as f32]);
        }
        assert_eq!(queue.len(), 4, "a stalled consumer must not build backlog");
        let mut out = [0.0; 4];
        assert_eq!(queue.pull(&mut out), 4);
        assert_eq!(out, [96.0, 97.0, 98.0, 99.0], "the newest audio survives");
    }

    #[test]
    fn target_depth_is_clamped_to_capacity() {
        let queue = PcmQueue::new(8);
        queue.set_target_depth(64);
        assert_eq!(queue.target_depth(), 8);
    }

    #[test]
    fn zero_target_depth_falls_back_to_capacity() {
        let queue = PcmQueue::new(4);
        queue.set_target_depth(2);
        queue.set_target_depth(0);
        queue.push(&[1.0, 2.0, 3.0, 4.0]);
        assert_eq!(queue.len(), 4);
    }

    #[test]
    fn pop_exact_timeout_expires_without_enough_samples() {
        let queue = PcmQueue::new(16);
        queue.push(&[1.0]);
        assert!(queue
            .pop_exact_timeout(4, Duration::from_millis(20))
            .is_none());
        // The partial frame is left intact for the next attempt.
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn pop_exact_timeout_wakes_on_push() {
        let queue = Arc::new(PcmQueue::new(16));
        let producer = Arc::clone(&queue);
        let waiter = std::thread::spawn(move || queue.pop_exact_timeout(4, Duration::from_secs(5)));
        // Generous timeout, but the condvar means the waiter returns as soon
        // as the frame completes rather than after the full wait.
        std::thread::sleep(Duration::from_millis(10));
        producer.push(&[1.0, 2.0, 3.0, 4.0]);
        assert_eq!(waiter.join().unwrap(), Some(vec![1.0, 2.0, 3.0, 4.0]));
    }
}
