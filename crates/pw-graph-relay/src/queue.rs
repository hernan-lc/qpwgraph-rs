//! PCM queues bridging realtime audio producers/consumers and the relay
//! engine's network worker threads.
//!
//! The PipeWire side may call `try_push`/`try_pull` from a realtime callback,
//! so those paths use `try_lock` and silently skip a quantum when the queue is
//! busy — the same conservative discipline `pipewire/effects.rs` applies to
//! processor state. Network workers use the blocking variants because a short
//! mutex hold on this side never risks an xrun.

use std::collections::VecDeque;
use std::sync::Mutex;

/// Default capacity: about 1.3 s of stereo 48 kHz audio in f32 samples.
pub const DEFAULT_QUEUE_CAPACITY: usize = 128 * 1024;

/// A bounded, drop-oldest FIFO of interleaved f32 samples.
pub struct PcmQueue {
    inner: Mutex<VecDeque<f32>>,
    capacity: usize,
}

impl PcmQueue {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(VecDeque::with_capacity(capacity)),
            capacity,
        }
    }

    /// Append samples, dropping the oldest samples if the queue would exceed
    /// its capacity. Fresh audio wins: relay latency matters more than
    /// completeness when a consumer stalls.
    pub fn push(&self, samples: &[f32]) {
        let Ok(mut queue) = self.inner.lock() else {
            return;
        };
        push_locked(&mut queue, samples, self.capacity);
    }

    /// Realtime-safe variant: never blocks. Returns `false` when the lock was
    /// busy and this quantum had to be skipped.
    pub fn try_push(&self, samples: &[f32]) -> bool {
        let Ok(mut queue) = self.inner.try_lock() else {
            return false;
        };
        push_locked(&mut queue, samples, self.capacity);
        true
    }

    /// Copy up to `out.len()` samples into `out`, returning how many were
    /// available. Remaining entries of `out` are left untouched.
    pub fn pull(&self, out: &mut [f32]) -> usize {
        let Ok(mut queue) = self.inner.lock() else {
            return 0;
        };
        let count = out.len().min(queue.len());
        for (slot, sample) in out.iter_mut().take(count).zip(queue.drain(..count)) {
            *slot = sample;
        }
        count
    }

    /// Realtime-safe variant of [`Self::pull`]. Returns 0 when the lock is busy.
    pub fn try_pull(&self, out: &mut [f32]) -> usize {
        let Ok(mut queue) = self.inner.try_lock() else {
            return 0;
        };
        let count = out.len().min(queue.len());
        for (slot, sample) in out.iter_mut().take(count).zip(queue.drain(..count)) {
            *slot = sample;
        }
        count
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

fn push_locked(queue: &mut VecDeque<f32>, samples: &[f32], capacity: usize) {
    if samples.len() >= capacity {
        // An oversized write keeps only its tail; history would be stale.
        queue.clear();
        queue.extend(samples.iter().copied().skip(samples.len() - capacity));
        return;
    }
    queue.extend(samples.iter().copied());
    while queue.len() > capacity {
        queue.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
