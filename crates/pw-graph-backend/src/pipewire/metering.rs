use super::*;

/// Live meter values shared between PipeWire's realtime data thread and the
/// thread that drives the UI. The callback only performs atomic stores.
#[derive(Debug)]
pub(super) struct MeterReadingState {
    rms_bits: AtomicU32,
    peak_bits: AtomicU32,
    pub(super) format: AtomicU32,
    pub(super) connected: AtomicBool,
    updated_at_ms: AtomicU64,
}

impl Default for MeterReadingState {
    fn default() -> Self {
        Self {
            rms_bits: AtomicU32::new(0),
            peak_bits: AtomicU32::new(0),
            format: AtomicU32::new(AudioFormat::F32LE.as_raw()),
            connected: AtomicBool::new(false),
            updated_at_ms: AtomicU64::new(u64::MAX),
        }
    }
}

impl MeterReadingState {
    pub(super) fn store_levels(&self, rms: f32, peak: f32, at_ms: u64) {
        self.rms_bits.store(rms.to_bits(), Ordering::Relaxed);
        self.peak_bits.store(peak.to_bits(), Ordering::Relaxed);
        self.updated_at_ms.store(at_ms, Ordering::Release);
    }

    pub(super) fn levels(&self, now_ms: u64) -> Option<(f32, f32, u32)> {
        let at_ms = self.updated_at_ms.load(Ordering::Acquire);
        if at_ms == u64::MAX {
            return None;
        }
        let age_ms = now_ms.saturating_sub(at_ms).min(u64::from(u32::MAX)) as u32;
        Some((
            f32::from_bits(self.rms_bits.load(Ordering::Relaxed)),
            f32::from_bits(self.peak_bits.load(Ordering::Relaxed)),
            age_ms,
        ))
    }
}

pub(super) struct MeterCallbackState {
    pub(super) shared: Arc<MeterReadingState>,
    pub(super) epoch: Instant,
}

pub(super) struct MeterHandle {
    pub(super) _stream: pw::stream::Stream,
    pub(super) _listener: pw::stream::StreamListener<MeterCallbackState>,
    pub(super) shared: Arc<MeterReadingState>,
}

pub(super) fn elapsed_ms_since(epoch: Instant) -> u64 {
    epoch.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

/// Process callback for the meter stream. It intentionally contains no
/// thread-loop access, locks, allocations, or UI interaction.
pub(super) fn process_meter_buffer(stream: &pw::stream::StreamRef, data: &mut MeterCallbackState) {
    let Some(mut buffer) = stream.dequeue_buffer() else {
        return;
    };
    let format = AudioFormat::from_raw(data.shared.format.load(Ordering::Relaxed));
    if !matches!(
        format,
        AudioFormat::F32LE | AudioFormat::F32BE | AudioFormat::F32P
    ) {
        return;
    }

    let mut peak = 0.0_f32;
    let mut sum = 0.0_f64;
    let mut samples = 0_u64;
    for block in buffer.datas_mut() {
        let offset = block.chunk().offset() as usize;
        let size = block.chunk().size() as usize;
        let Some(bytes) = block.data() else {
            continue;
        };
        let end = offset.saturating_add(size).min(bytes.len());
        if offset >= end {
            continue;
        }
        for chunk in bytes[offset..end]
            .as_chunks::<{ std::mem::size_of::<f32>() }>()
            .0
        {
            let mut raw = [0_u8; 4];
            raw.copy_from_slice(chunk);
            let value = if format == AudioFormat::F32BE {
                f32::from_be_bytes(raw)
            } else {
                f32::from_le_bytes(raw)
            };
            if !value.is_finite() {
                continue;
            }
            let absolute = value.abs();
            peak = peak.max(absolute);
            sum += f64::from(absolute) * f64::from(absolute);
            samples += 1;
        }
    }

    if samples > 0 {
        data.shared.store_levels(
            (sum / samples as f64).sqrt().min(1.0) as f32,
            peak.min(1.0),
            elapsed_ms_since(data.epoch),
        );
    }
}
