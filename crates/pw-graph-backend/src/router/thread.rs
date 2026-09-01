//! Running the router on its own thread.
//!
//! [`RouterCore`] is deliberately synchronous so its semantics can be tested
//! without a clock. In production something has to turn the crank, and that is
//! this: one thread, paced to the block duration, pulling from the device
//! rings and pushing to them.
//!
//! Control operations run on that same thread, between blocks, as closures
//! sent over a channel. That is not a real-time violation -- the thread is not
//! a device callback, and devices keep filling their rings across a pause of a
//! few microseconds -- and it means [`RouterCore`] needs no interior locking
//! at all. What it does mean is that a closure sent here must not block: it
//! runs where audio is waiting.
//!
//! Anything the core hands back on removal is dropped by the *caller*, not
//! here, so a device's teardown never happens between two blocks.

use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::engine::{ProcessReport, RouterConfig, RouterCore};

/// The router thread stopped, so the operation could not be run.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("the audio router thread is not running")]
pub struct RouterStopped;

type Job = Box<dyn FnOnce(&mut RouterCore) + Send>;

enum Message {
    Run(Job),
    Stop,
}

/// A [`RouterCore`] running on a paced thread.
pub struct RouterThread {
    commands: Sender<Message>,
    worker: Option<JoinHandle<()>>,
    config: RouterConfig,
}

impl RouterThread {
    /// Start the router.
    ///
    /// The thread is named so it is identifiable in a debugger and in a crash
    /// dump, which is the only way to tell an audio stall from a UI stall
    /// after the fact.
    pub fn start(config: RouterConfig) -> std::io::Result<Self> {
        let (commands, inbox) = mpsc::channel::<Message>();
        let worker = thread::Builder::new()
            .name("qpwgraph-audio-router".into())
            .spawn(move || {
                let mut core = RouterCore::new(config);
                let period = block_period(config);
                let mut deadline = Instant::now();
                loop {
                    // Absolute deadlines rather than sleeping for a period:
                    // sleeping accumulates every scheduling overshoot into a
                    // permanent lag against the device clocks.
                    deadline += period;
                    let now = Instant::now();
                    let wait = deadline.saturating_duration_since(now);
                    if wait.is_zero() && now.saturating_duration_since(deadline) > period {
                        // Badly overshot -- a suspend, or the machine was
                        // simply busy. Re-anchor instead of spinning through
                        // a burst of catch-up blocks.
                        deadline = now;
                    }
                    match inbox.recv_timeout(wait) {
                        Ok(Message::Run(job)) => {
                            job(&mut core);
                            // A control operation is not a reason to skip a
                            // block, so fall through to the next wait rather
                            // than processing early.
                            deadline -= period;
                            continue;
                        }
                        Ok(Message::Stop) | Err(RecvTimeoutError::Disconnected) => break,
                        Err(RecvTimeoutError::Timeout) => {}
                    }
                    core.process();
                }
            })?;
        Ok(Self {
            commands,
            worker: Some(worker),
            config,
        })
    }

    pub fn config(&self) -> RouterConfig {
        self.config
    }

    /// Run `job` against the core between two blocks and wait for its result.
    ///
    /// This is how every control operation reaches the router: registering a
    /// device, replacing the route table, changing an effect parameter. The
    /// result comes back by value, so anything the core gives up -- a removed
    /// source, a retired route table -- is dropped on this thread.
    pub fn with<T, F>(&self, job: F) -> Result<T, RouterStopped>
    where
        T: Send + 'static,
        F: FnOnce(&mut RouterCore) -> T + Send + 'static,
    {
        let (reply, answer) = mpsc::channel();
        self.commands
            .send(Message::Run(Box::new(move |core| {
                let _ = reply.send(job(core));
            })))
            .map_err(|_| RouterStopped)?;
        answer.recv().map_err(|_| RouterStopped)
    }

    /// Drive one block by hand.
    ///
    /// For tests and for a caller that wants to step the router
    /// deterministically; the paced loop keeps running either way, so this is
    /// an extra block rather than the only one.
    pub fn step(&self) -> Result<ProcessReport, RouterStopped> {
        self.with(|core| core.process())
    }

    /// Stop the thread and wait for it.
    pub fn shutdown(&mut self) {
        let _ = self.commands.send(Message::Stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for RouterThread {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl std::fmt::Debug for RouterThread {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RouterThread")
            .field("block_frames", &self.config.block_frames)
            .field("running", &self.worker.is_some())
            .finish()
    }
}

/// How long one block of audio lasts.
fn block_period(config: RouterConfig) -> Duration {
    if config.clock_rate == 0 {
        return Duration::from_millis(10);
    }
    Duration::from_nanos(
        (config.block_frames as u64 * 1_000_000_000) / u64::from(config.clock_rate),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router::endpoints::{BufferSource, CaptureSink};
    use crate::router::engine::{RouteId, RouteSpec, SinkId, SourceId};
    use crate::router::format::AudioFormat;

    const MONO: AudioFormat = AudioFormat::new(48_000, 1);

    fn config() -> RouterConfig {
        RouterConfig {
            block_frames: 4,
            clock_rate: 48_000,
        }
    }

    #[test]
    fn a_block_period_is_the_block_length_at_the_clock_rate() {
        assert_eq!(
            block_period(RouterConfig {
                block_frames: 480,
                clock_rate: 48_000,
            }),
            Duration::from_millis(10)
        );
    }

    #[test]
    fn a_zero_clock_rate_falls_back_instead_of_dividing_by_zero() {
        assert_eq!(
            block_period(RouterConfig {
                block_frames: 480,
                clock_rate: 0,
            }),
            Duration::from_millis(10)
        );
    }

    #[test]
    fn control_operations_run_against_the_core_and_return_their_result() {
        let router = RouterThread::start(config()).expect("the router thread starts");
        let added = router
            .with(|core| {
                core.add_source(
                    SourceId(1),
                    Box::new(BufferSource::looping(MONO, vec![0.5])),
                )
            })
            .expect("the router is running");
        assert!(added.is_ok());
        let ids = router.with(|core| core.route_ids()).expect("still running");
        assert!(ids.is_empty());
    }

    #[test]
    fn the_paced_loop_carries_audio_without_anyone_stepping_it() {
        let router = RouterThread::start(config()).expect("the router thread starts");
        let (sink, captured) = CaptureSink::new(MONO);
        router
            .with(move |core| {
                core.add_source(
                    SourceId(1),
                    Box::new(BufferSource::looping(MONO, vec![0.5])),
                )
                .expect("a fresh id");
                core.add_sink(SinkId(1), Box::new(sink))
                    .expect("a fresh id");
                core.set_routes(&[RouteSpec::direct(RouteId(1), SourceId(1), SinkId(1))])
                    .expect("a valid route");
            })
            .expect("the router is running");

        // The loop paces itself, so this waits on audio arriving rather than
        // on a fixed number of steps.
        let deadline = Instant::now() + Duration::from_secs(5);
        while captured.lock().unwrap().is_empty() {
            assert!(Instant::now() < deadline, "the router never carried audio");
            thread::sleep(Duration::from_millis(1));
        }
        assert!(captured.lock().unwrap().iter().all(|&s| s == 0.5));
    }

    #[test]
    fn a_stopped_router_reports_that_it_is_gone_rather_than_hanging() {
        let mut router = RouterThread::start(config()).expect("the router thread starts");
        router.shutdown();
        assert_eq!(router.with(|core| core.frame_clock()), Err(RouterStopped));
    }
}
