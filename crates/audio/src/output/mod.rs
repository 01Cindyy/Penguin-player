//! Audio output: an output stream (the sound device, or a null sink) together with the
//! writer end of the ring buffer that feeds it.

mod device;
mod null;

use std::sync::Arc;
use std::time::Duration;

use cpal::SampleFormat;

use crate::AudioError;
use crate::ring::{RAMP, RingWriter, ring};
use crate::shared::Shared;

use device::DeviceSink;
use null::NullStream;

/// Seconds of decoded audio the engine keeps queued ahead of the output.
const FILL_TARGET: f64 = 0.4;
/// The engine tops the queue up when it falls below this many seconds.
const REFILL_BELOW: f64 = 0.25;
/// Ring buffer capacity in seconds. Audio decoded after a flush that does not fit yet waits
/// in the writer's backlog until the output has dropped the old audio.
const RING_CAPACITY: f64 = 0.5;
/// Extra time allowed for a fade-out to reach the speakers before the device is paused.
const FADE_MARGIN: Duration = Duration::from_millis(30);
/// Longest wait for a fade-out before closing a stream anyway.
const CLOSE_TIMEOUT: Duration = Duration::from_millis(150);

/// The format an output stream runs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OutputFormat {
    pub sample_rate: u32,
    pub channels: u16,
    pub sample_format: SampleFormat,
}

/// Where audio goes: the system's default output device, or nowhere.
pub(crate) enum Sink {
    Device(DeviceSink),
    Null,
}

impl Sink {
    pub fn open(null_output: bool) -> Result<Self, AudioError> {
        if null_output {
            Ok(Sink::Null)
        } else {
            DeviceSink::open_default().map(Sink::Device)
        }
    }

    /// The format to play audio with the given sample rate in: the audio's own rate whenever
    /// the device supports it, so nothing is resampled.
    pub fn format_for(&self, sample_rate: u32) -> OutputFormat {
        match self {
            Sink::Device(device) => device.format_for(sample_rate),
            Sink::Null => OutputFormat {
                sample_rate,
                channels: 2,
                sample_format: SampleFormat::F32,
            },
        }
    }

    /// Opens a stream in `format`; it starts paused. `id` identifies the stream in
    /// `on_error`, which is called from another thread if the stream fails while running.
    pub fn open_output(
        &self,
        format: OutputFormat,
        shared: &Arc<Shared>,
        id: u64,
        on_error: impl FnMut(u64, String) + Send + 'static,
    ) -> Result<Output, AudioError> {
        let samples_per_second = f64::from(format.sample_rate) * f64::from(format.channels);
        let capacity = (samples_per_second * RING_CAPACITY) as usize;
        let (writer, reader) = ring(
            capacity,
            usize::from(format.channels),
            format.sample_rate,
            Arc::clone(shared),
        );
        let stream = match self {
            Sink::Device(device) => {
                Stream::Device(device.build(format, reader, Arc::clone(shared), id, on_error)?)
            }
            Sink::Null => Stream::Null(NullStream::start(format.sample_rate, reader)?),
        };
        log::debug!("opened output {id}: {format:?}");
        Ok(Output {
            id,
            format,
            stream,
            writer,
            shared: Arc::clone(shared),
            running: false,
            samples_per_second,
        })
    }
}

/// Channel count to open a stream with, given which counts the sink supports: stereo, or
/// mono for a mono-only device. Mono tracks play on both speakers and surround is folded
/// down to stereo, because speaker order differs between platforms' audio APIs and stereo
/// is what music is listened on. `None` if the sink supports neither.
pub(crate) fn output_channels(supports: impl Fn(u16) -> bool) -> Option<u16> {
    [2, 1].into_iter().find(|&count| supports(count))
}

enum Stream {
    Device(cpal::Stream),
    Null(NullStream),
}

/// An open output stream and the ring buffer that feeds it.
pub(crate) struct Output {
    id: u64,
    format: OutputFormat,
    stream: Stream,
    pub writer: RingWriter,
    shared: Arc<Shared>,
    /// The stream has been started and not paused since.
    running: bool,
    samples_per_second: f64,
}

impl Output {
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn format(&self) -> OutputFormat {
        self.format
    }

    /// Whether the queue is below the fill target, so more audio should be decoded.
    pub fn needs_audio(&self) -> bool {
        (self.writer.queued() as f64) < FILL_TARGET * self.samples_per_second
    }

    /// How long until the output has consumed up to sample index `sample`.
    pub fn time_until(&self, sample: u64) -> Duration {
        self.duration_of(sample.saturating_sub(self.writer.consumed()) as f64)
    }

    /// How long until the queue falls low enough to be topped up.
    pub fn time_until_refill(&self) -> Duration {
        self.duration_of(self.writer.queued() as f64 - REFILL_BELOW * self.samples_per_second)
    }

    /// Starts (or resumes) the stream and fades the audio in.
    pub fn play(&mut self) -> Result<(), AudioError> {
        self.shared.set_playing(true);
        if !self.running {
            match &self.stream {
                Stream::Device(stream) => device::play(stream)?,
                Stream::Null(stream) => stream.play(),
            }
            self.running = true;
        }
        Ok(())
    }

    /// Starts fading the audio out. Returns how long until the fade has been heard, after
    /// which [`Output::suspend`] can pause the device without a click.
    pub fn fade_out(&self) -> Duration {
        self.shared.set_playing(false);
        RAMP + self.shared.latency() + FADE_MARGIN
    }

    /// Pauses the stream so the device idles. A device that cannot pause keeps running and
    /// plays silence.
    pub fn suspend(&mut self) {
        if !self.running {
            return;
        }
        self.running = match &self.stream {
            Stream::Device(stream) => !device::pause(stream),
            Stream::Null(stream) => {
                stream.pause();
                false
            }
        };
    }

    /// Fades out and closes the stream, waiting briefly so the fade is heard.
    pub fn close(self) {
        log::debug!(
            "closing output {} (device latency {:?})",
            self.id,
            self.shared.latency()
        );
        if self.running && !self.shared.is_silent() {
            std::thread::sleep(self.fade_out().min(CLOSE_TIMEOUT));
        }
    }

    fn duration_of(&self, samples: f64) -> Duration {
        Duration::from_secs_f64((samples / self.samples_per_second).max(0.0))
    }
}
