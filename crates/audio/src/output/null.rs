//! An output without a device: a timer thread consumes the audio in real time and discards
//! it. Used for tests and for running without sound.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::AudioError;
use crate::ring::RingReader;
use crate::shared::frames_to_nanos;

/// Audio consumed per tick, like one device period.
const TICK: Duration = Duration::from_millis(10);
/// If the timer falls further behind than this (the machine was suspended, say), it starts
/// afresh instead of catching up in a burst.
const MAX_LAG: Duration = Duration::from_millis(200);

#[derive(Default)]
struct Control {
    running: AtomicBool,
    shutdown: AtomicBool,
}

pub(crate) struct NullStream {
    control: Arc<Control>,
    thread: Option<JoinHandle<()>>,
}

impl NullStream {
    /// Starts the timer thread, paused.
    pub fn start(sample_rate: u32, reader: RingReader) -> Result<Self, AudioError> {
        let control = Arc::new(Control::default());
        let thread = thread::Builder::new()
            .name("mp-audio-null".into())
            .spawn({
                let control = Arc::clone(&control);
                move || run(reader, sample_rate.max(1), &control)
            })
            .map_err(|error| AudioError::Output(error.to_string()))?;
        Ok(Self {
            control,
            thread: Some(thread),
        })
    }

    pub fn play(&self) {
        self.control.running.store(true, Ordering::Release);
        self.wake();
    }

    pub fn pause(&self) {
        self.control.running.store(false, Ordering::Release);
    }

    fn wake(&self) {
        if let Some(thread) = &self.thread {
            thread.thread().unpark();
        }
    }
}

impl Drop for NullStream {
    fn drop(&mut self) {
        self.control.shutdown.store(true, Ordering::Release);
        self.wake();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Renders one tick of audio at a time, keeping to the wall clock. Each tick's due time is
/// computed from the total frames rendered, so rounding never accumulates.
fn run(mut reader: RingReader, sample_rate: u32, control: &Control) {
    let rate = u64::from(sample_rate);
    let tick_frames = (rate * TICK.as_millis() as u64 / 1000).max(1);
    let mut buffer = vec![0.0; tick_frames as usize * reader.channels()];
    let mut started = Instant::now();
    let mut frames = 0_u64;

    while !control.shutdown.load(Ordering::Acquire) {
        if !control.running.load(Ordering::Acquire) {
            thread::park();
            started = Instant::now();
            frames = 0;
            continue;
        }
        let due = started + Duration::from_nanos(frames_to_nanos(frames, rate));
        let now = Instant::now();
        if due > now {
            // Woken early (or spuriously): check the flags again, then keep waiting.
            thread::park_timeout(due - now);
            continue;
        }
        if now - due > MAX_LAG {
            started = now;
            frames = 0;
        }
        reader.render(&mut buffer);
        frames += tick_frames;
    }
}
