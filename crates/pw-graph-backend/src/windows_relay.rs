//! WASAPI audio endpoints for the relay on Windows.
//!
//! On Linux the relay owns two virtual PipeWire nodes, so any application can
//! be routed into or out of it through the patchbay. Windows has no user-mode
//! API for creating an audio endpoint — a device other applications can select
//! requires a kernel-mode driver — so the relay is wired to fixed endpoints
//! instead:
//!
//! * **Capture** loopback-records the selected playback endpoint, so whatever
//!   that endpoint is playing is what peers receive.
//! * **Render** plays audio received from peers on the selected playback
//!   endpoint.
//!
//! That covers "use my phone as a speaker" and "play the phone's audio here".
//! It cannot cover "use my phone as a microphone for other Windows apps",
//! because the received audio would have to appear as a capture device, which
//! is exactly the thing user-mode code cannot create. The relay protocol can
//! still receive and render peer audio locally; only microphone emulation is
//! unavailable.
//!
//! Both loops poll rather than waiting on an event handle: loopback capture
//! does not raise the WASAPI event, so a single polling shape keeps the two
//! threads symmetrical.

use crate::api::{BackendError, BackendResult};
use pw_graph_relay::{EngineConfig, RelayEngine, RelayHandle};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use windows::Win32::Media::Audio;
use windows::Win32::System::Com::{self, CLSCTX_ALL, COINIT_MULTITHREADED};

/// The relay engine's PCM format. WASAPI is asked to convert to it, so the
/// endpoint's own mix format never leaks into the wire format.
pub(crate) const RELAY_SAMPLE_RATE: u32 = 48_000;
pub(crate) const RELAY_CHANNELS: u16 = 2;

/// `WAVEFORMATEX.wFormatTag` for 32-bit float samples.
const WAVE_FORMAT_IEEE_FLOAT: u16 = 0x0003;
/// Endpoint buffer, in 100 ns units. 40 ms is comfortably above the shared
/// mode period on every device tested and keeps the poll loop cheap.
const BUFFER_DURATION_HNS: i64 = 400_000;
/// Poll interval. Well under the buffer duration so neither side starves.
const POLL_INTERVAL: Duration = Duration::from_millis(5);
/// How long `start` waits for each endpoint to report that WASAPI accepted it.
const ENDPOINT_START_TIMEOUT: Duration = Duration::from_secs(5);

/// Which endpoints the relay uses, by Core Audio device id.
///
/// `None` means the current default playback endpoint, which is also what a
/// removed or unplugged device falls back to. Ids come straight from the
/// driver's endpoint enumeration, so the UI can offer the same list it already
/// draws as graph nodes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RelayEndpoints {
    /// Endpoint whose loopback is sent to peers.
    pub capture: Option<String>,
    /// Endpoint that peer audio is played on.
    pub playback: Option<String>,
}

/// The relay engine plus the two WASAPI threads that feed and drain it.
///
/// Dropping this stops both threads and the engine. The struct is deliberately
/// the only owner of the `RelayEngine`, so the endpoints cannot outlive it.
pub(crate) struct WindowsRelayDevices {
    engine: RelayEngine,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
    endpoints: RelayEndpoints,
}

impl WindowsRelayDevices {
    /// The endpoints this instance was started with. Changing them means
    /// restarting the devices, because a WASAPI client is bound to its device.
    pub(crate) fn endpoints(&self) -> &RelayEndpoints {
        &self.endpoints
    }

    pub(crate) fn start(config: EngineConfig, endpoints: RelayEndpoints) -> BackendResult<Self> {
        let engine = RelayEngine::start(config)
            .map_err(|error| BackendError::native(format!("relay engine start failed: {error}")))?;
        let stop = Arc::new(AtomicBool::new(false));

        let mut threads = Vec::with_capacity(2);
        let mut ready = Vec::with_capacity(2);
        for (name, direction, device) in [
            (
                "qpwgraph-relay-capture",
                Direction::Capture,
                endpoints.capture.clone(),
            ),
            (
                "qpwgraph-relay-render",
                Direction::Render,
                endpoints.playback.clone(),
            ),
        ] {
            let (thread, started) = match spawn_endpoint_thread(
                name,
                engine.handle(),
                Arc::clone(&stop),
                direction,
                device,
            ) {
                Ok(result) => result,
                Err(error) => {
                    stop_threads(&stop, &mut threads);
                    engine.shutdown();
                    return Err(error);
                }
            };
            threads.push(thread);
            ready.push(started);
        }

        // Wait for both endpoints to report that WASAPI accepted them. Without
        // this a failure inside a thread would leave a host that looks started
        // but never carries audio, with nothing to explain why.
        for started in ready {
            let result = match started.recv_timeout(ENDPOINT_START_TIMEOUT) {
                Ok(result) => result,
                Err(_) => Err(BackendError::native(
                    "Windows relay endpoint did not start in time",
                )),
            };
            if let Err(error) = result {
                stop_threads(&stop, &mut threads);
                engine.shutdown();
                return Err(error);
            }
        }
        Ok(Self {
            engine,
            stop,
            threads,
            endpoints,
        })
    }

    pub(crate) fn handle(&self) -> RelayHandle {
        self.engine.handle()
    }
}

impl Drop for WindowsRelayDevices {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
        self.engine.shutdown();
    }
}

impl std::fmt::Debug for WindowsRelayDevices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowsRelayDevices")
            .field("endpoint_threads", &self.threads.len())
            .field("stopping", &self.stop.load(Ordering::Acquire))
            .finish()
    }
}

#[derive(Clone, Copy)]
enum Direction {
    /// Loopback-record the selected playback endpoint into the engine.
    Capture,
    /// Play what the engine received on the selected playback endpoint.
    Render,
}

type StartResult = Receiver<BackendResult<()>>;

fn spawn_endpoint_thread(
    name: &str,
    handle: RelayHandle,
    stop: Arc<AtomicBool>,
    direction: Direction,
    device_id: Option<String>,
) -> BackendResult<(JoinHandle<()>, StartResult)> {
    let (started_tx, started_rx) = mpsc::channel();
    let thread = thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            // Every COM apartment is per-thread, so each endpoint initializes
            // its own and tears it down on the way out.
            let initialized = unsafe { Com::CoInitializeEx(None, COINIT_MULTITHREADED) };
            if initialized.is_err() {
                let _ = started_tx.send(Err(BackendError::native(format!(
                    "could not initialize COM for the relay endpoint: {initialized:?}"
                ))));
                return;
            }
            run_endpoint(&handle, &stop, direction, device_id.as_deref(), started_tx);
            unsafe { Com::CoUninitialize() };
        })
        .map_err(|error| {
            BackendError::native(format!("could not start relay endpoint: {error}"))
        })?;
    Ok((thread, started_rx))
}

fn stop_threads(stop: &Arc<AtomicBool>, threads: &mut Vec<JoinHandle<()>>) {
    stop.store(true, Ordering::Release);
    for thread in threads.drain(..) {
        let _ = thread.join();
    }
}

/// Interleaved 48 kHz stereo float, which is what the engine speaks.
fn relay_format() -> Audio::WAVEFORMATEX {
    let channels = RELAY_CHANNELS;
    let bits = 32u16;
    let block_align = channels * bits / 8;
    Audio::WAVEFORMATEX {
        wFormatTag: WAVE_FORMAT_IEEE_FLOAT,
        nChannels: channels,
        nSamplesPerSec: RELAY_SAMPLE_RATE,
        nAvgBytesPerSec: RELAY_SAMPLE_RATE * u32::from(block_align),
        nBlockAlign: block_align,
        wBitsPerSample: bits,
        cbSize: 0,
    }
}

/// Open the endpoint, report whether WASAPI accepted it, then run until asked
/// to stop. The report is sent exactly once, so `start` can surface a real
/// error instead of leaving a silent host behind.
fn run_endpoint(
    handle: &RelayHandle,
    stop: &Arc<AtomicBool>,
    direction: Direction,
    device_id: Option<&str>,
    started: Sender<BackendResult<()>>,
) {
    // The service is acquired here, before the endpoint reports success.
    // Doing it inside the loop hid a real failure: `GetService` was being
    // called as `cast`, which returns E_NOINTERFACE, so both loops exited
    // immediately and the relay carried no audio while still looking started.
    let opened = open_endpoint(direction, device_id).and_then(|client| match direction {
        Direction::Capture => unsafe { client.GetService::<Audio::IAudioCaptureClient>() }
            .map(|service| (client, Service::Capture(service)))
            .map_err(|error| native("get capture service", error)),
        Direction::Render => unsafe { client.GetService::<Audio::IAudioRenderClient>() }
            .map(|service| (client, Service::Render(service)))
            .map_err(|error| native("get render service", error)),
    });
    match opened {
        Ok((client, service)) => {
            if let Err(error) = unsafe { client.Start() } {
                let _ = started.send(Err(native("start audio client", error)));
                return;
            }
            let _ = started.send(Ok(()));
            match service {
                Service::Capture(capture) => capture_loop(&capture, handle, stop),
                Service::Render(render) => render_loop(&client, &render, handle, stop),
            }
            if let Err(error) = unsafe { client.Stop() } {
                report_endpoint_error(handle, "stop audio client", error);
            }
        }
        Err(error) => {
            let _ = started.send(Err(error));
        }
    }
}

enum Service {
    Capture(Audio::IAudioCaptureClient),
    Render(Audio::IAudioRenderClient),
}

fn open_endpoint(
    direction: Direction,
    device_id: Option<&str>,
) -> BackendResult<Audio::IAudioClient> {
    let enumerator: Audio::IMMDeviceEnumerator =
        unsafe { Com::CoCreateInstance(&Audio::MMDeviceEnumerator, None, CLSCTX_ALL) }
            .map_err(|error| native("create MMDeviceEnumerator", error))?;
    // Both directions target a render endpoint: capture reads its loopback.
    // A named device that has since been unplugged falls back to the default
    // rather than failing the whole relay.
    let device = match device_id {
        Some(id) => {
            let wide: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
            unsafe { enumerator.GetDevice(windows::core::PCWSTR(wide.as_ptr())) }.ok()
        }
        None => None,
    };
    let device = match device {
        Some(device) => device,
        None => unsafe { enumerator.GetDefaultAudioEndpoint(Audio::eRender, Audio::eConsole) }
            .map_err(|error| native("open default playback endpoint", error))?,
    };
    let client: Audio::IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None) }
        .map_err(|error| native("activate audio client", error))?;

    let format = relay_format();
    // AUTOCONVERTPCM makes the audio engine resample and remix for us, so an
    // endpoint running at 44.1 kHz or 7.1 still yields the relay's format and
    // no resampler is needed here.
    let mut flags =
        Audio::AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | Audio::AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
    if matches!(direction, Direction::Capture) {
        flags |= Audio::AUDCLNT_STREAMFLAGS_LOOPBACK;
    }
    unsafe {
        client.Initialize(
            Audio::AUDCLNT_SHAREMODE_SHARED,
            flags,
            BUFFER_DURATION_HNS,
            0,
            &format,
            None,
        )
    }
    .map_err(|error| native("initialize audio client", error))?;

    Ok(client)
}

/// Loopback-record the playback endpoint and hand every frame to the engine.
fn capture_loop(
    capture: &Audio::IAudioCaptureClient,
    handle: &RelayHandle,
    stop: &Arc<AtomicBool>,
) {
    let channels = usize::from(RELAY_CHANNELS);

    while !stop.load(Ordering::Acquire) {
        let mut pending = match unsafe { capture.GetNextPacketSize() } {
            Ok(pending) => pending,
            Err(error) => {
                report_endpoint_error(handle, "read capture packet size", error);
                break;
            }
        };
        if pending == 0 {
            thread::sleep(POLL_INTERVAL);
            continue;
        }
        while pending > 0 && !stop.load(Ordering::Acquire) {
            let mut data = std::ptr::null_mut();
            let mut frames = 0u32;
            let mut buffer_flags = 0u32;
            if let Err(error) =
                unsafe { capture.GetBuffer(&mut data, &mut frames, &mut buffer_flags, None, None) }
            {
                report_endpoint_error(handle, "read capture buffer", error);
                break;
            }
            if frames > 0 {
                if buffer_flags & Audio::AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 {
                    // WASAPI is allowed to hand back a silent packet without
                    // filling the buffer, so synthesise the silence instead of
                    // reading whatever the pointer happens to hold.
                    let silence = vec![0.0f32; frames as usize * channels];
                    handle.push_capture(&silence);
                } else if !data.is_null() {
                    let samples = unsafe {
                        std::slice::from_raw_parts(data.cast::<f32>(), frames as usize * channels)
                    };
                    handle.push_capture(samples);
                } else {
                    handle.report_error("Windows relay capture endpoint returned a null buffer");
                    break;
                }
            }
            if let Err(error) = unsafe { capture.ReleaseBuffer(frames) } {
                report_endpoint_error(handle, "release capture buffer", error);
                break;
            }
            pending = match unsafe { capture.GetNextPacketSize() } {
                Ok(pending) => pending,
                Err(error) => {
                    report_endpoint_error(handle, "read capture packet size", error);
                    break;
                }
            };
        }
    }
}

/// Drain audio received from peers onto the playback endpoint.
fn render_loop(
    client: &Audio::IAudioClient,
    render: &Audio::IAudioRenderClient,
    handle: &RelayHandle,
    stop: &Arc<AtomicBool>,
) {
    let buffer_frames = match unsafe { client.GetBufferSize() } {
        Ok(buffer_frames) => buffer_frames,
        Err(error) => {
            report_endpoint_error(handle, "read render buffer size", error);
            return;
        }
    };
    if buffer_frames == 0 {
        handle.report_error("Windows relay render endpoint returned a zero-sized buffer");
        return;
    }
    let channels = usize::from(RELAY_CHANNELS);
    let mut scratch = vec![0.0f32; buffer_frames as usize * channels];

    while !stop.load(Ordering::Acquire) {
        let padding = match unsafe { client.GetCurrentPadding() } {
            Ok(padding) => padding,
            Err(error) => {
                report_endpoint_error(handle, "read render padding", error);
                break;
            }
        };
        let available = buffer_frames.saturating_sub(padding);
        if available == 0 {
            thread::sleep(POLL_INTERVAL);
            continue;
        }
        let wanted = available as usize * channels;
        let filled = handle.pull_playback(&mut scratch[..wanted]);
        // The engine reports how much it had. Anything short is silence, so a
        // starved session plays quiet rather than repeating the last buffer.
        if filled < wanted {
            scratch[filled..wanted].fill(0.0);
        }

        let data = match unsafe { render.GetBuffer(available) } {
            Ok(data) => data,
            Err(error) => {
                report_endpoint_error(handle, "acquire render buffer", error);
                break;
            }
        };
        if data.is_null() {
            handle.report_error("Windows relay render endpoint returned a null buffer");
            break;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(scratch.as_ptr(), data.cast::<f32>(), wanted);
            if let Err(error) = render.ReleaseBuffer(available, 0) {
                report_endpoint_error(handle, "release render buffer", error);
                break;
            }
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn report_endpoint_error(handle: &RelayHandle, operation: &str, error: windows::core::Error) {
    handle.report_error(format!("Windows relay {operation} failed: {error}"));
}

fn native(context: &str, error: windows::core::Error) -> BackendError {
    BackendError::native(format!("Windows relay {context} failed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_relay_format_describes_interleaved_stereo_float() {
        let format = relay_format();
        // WAVEFORMATEX is packed, so each field has to be copied out before it
        // can be compared.
        let (tag, channels) = (format.wFormatTag, format.nChannels);
        let (rate, bits) = (format.nSamplesPerSec, format.wBitsPerSample);
        let (block_align, avg_bytes) = (format.nBlockAlign, format.nAvgBytesPerSec);

        assert_eq!(tag, WAVE_FORMAT_IEEE_FLOAT);
        assert_eq!(channels, RELAY_CHANNELS);
        assert_eq!(rate, RELAY_SAMPLE_RATE);
        assert_eq!(bits, 32);
        // A frame is one sample per channel; getting this wrong makes WASAPI
        // walk the buffer at the wrong stride, so the audio comes out
        // pitch-shifted rather than failing outright.
        assert_eq!(block_align, RELAY_CHANNELS * 4);
        assert_eq!(avg_bytes, RELAY_SAMPLE_RATE * u32::from(block_align));
    }
}
