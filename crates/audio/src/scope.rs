//! A copy of the audio as it reaches the speakers, for drawing a visualiser.
//!
//! The output callback writes a mono mix of every block it renders into a ring of atomics,
//! so it never waits on the reader. The reader takes the most recent frames, corrected for
//! the device's latency, and may see a block being overwritten only if it reads a third of
//! a second late, which a visualiser doesn't mind.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::shared::{Shared, duration_to_nanos};

/// Mono frames kept: a third of a second even at 192 kHz.
const CAPACITY: usize = 1 << 16;
/// Readers stay this far from the oldest frames, which the writer overwrites next.
const MARGIN: usize = CAPACITY / 2;
/// Without new audio for this long, playback has stopped and there is nothing to show.
const STALE_AFTER: Duration = Duration::from_millis(250);

pub(crate) struct ScopeBuffer {
    samples: Box<[AtomicU32]>,
    /// Frames written so far.
    written: AtomicU64,
    /// When the last block was written, in nanoseconds since `epoch`.
    written_at_ns: AtomicU64,
    sample_rate: AtomicU32,
    /// How many visualisers are watching; while none are, the output copies nothing.
    watchers: AtomicUsize,
    epoch: Instant,
}

impl ScopeBuffer {
    pub fn new() -> Self {
        Self {
            samples: (0..CAPACITY).map(|_| AtomicU32::new(0)).collect(),
            written: AtomicU64::new(0),
            written_at_ns: AtomicU64::new(0),
            sample_rate: AtomicU32::new(0),
            watchers: AtomicUsize::new(0),
            epoch: Instant::now(),
        }
    }

    /// Called by the output callback with each block it rendered. Never blocks.
    pub fn push(&self, interleaved: &[f32], channels: usize, sample_rate: u32) {
        if self.watchers.load(Ordering::Relaxed) == 0 || channels == 0 {
            return;
        }
        let mut written = self.written.load(Ordering::Relaxed);
        let scale = 1.0 / channels as f32;
        for frame in interleaved.chunks_exact(channels) {
            let mono = frame.iter().sum::<f32>() * scale;
            self.samples[written as usize % CAPACITY].store(mono.to_bits(), Ordering::Relaxed);
            written += 1;
        }
        self.sample_rate.store(sample_rate, Ordering::Relaxed);
        self.written_at_ns
            .store(duration_to_nanos(self.epoch.elapsed()), Ordering::Relaxed);
        self.written.store(written, Ordering::Release);
    }
}

/// The audio the speakers are playing right now, for a visualiser. Cheap to clone.
#[derive(Clone)]
pub struct Scope {
    shared: Arc<Shared>,
}

impl Scope {
    pub(crate) fn new(shared: Arc<Shared>) -> Self {
        Self { shared }
    }

    /// Starts copying audio for the scope until the returned watch is dropped. Nothing
    /// is copied while nobody watches, so playback does no extra work then.
    pub fn watch(&self) -> ScopeWatch {
        self.buffer().watchers.fetch_add(1, Ordering::Relaxed);
        ScopeWatch {
            shared: self.shared.clone(),
        }
    }

    /// Sample rate of the audio being played, `0` before anything has played.
    pub fn sample_rate(&self) -> u32 {
        self.buffer().sample_rate.load(Ordering::Relaxed)
    }

    /// Fills `out` with the mono audio that ends where the speakers are now. Returns
    /// `false`, leaving `out` alone, when nothing has played for a moment.
    pub fn latest(&self, out: &mut [f32]) -> bool {
        let buffer = self.buffer();
        let written = buffer.written.load(Ordering::Acquire);
        let rate = f64::from(buffer.sample_rate.load(Ordering::Relaxed));
        let since_write = buffer.epoch.elapsed().saturating_sub(Duration::from_nanos(
            buffer.written_at_ns.load(Ordering::Relaxed),
        ));
        if written == 0 || rate == 0.0 || since_write > STALE_AFTER || out.len() > MARGIN {
            return false;
        }
        // The device plays each block `latency` after it was written, and keeps playing
        // between callbacks, so estimate how far it has got since the last one.
        let ahead = (since_write.as_secs_f64() - self.shared.latency().as_secs_f64()) * rate;
        let end = (written as f64 + ahead).clamp(0.0, written as f64) as u64;
        let oldest_safe = written.saturating_sub(MARGIN as u64);
        let start = end.saturating_sub(out.len() as u64).max(oldest_safe);
        let available = (end - start) as usize;
        let (silence, audio) = out.split_at_mut(out.len() - available);
        silence.fill(0.0);
        for (index, sample) in (start..end).zip(audio) {
            *sample =
                f32::from_bits(buffer.samples[index as usize % CAPACITY].load(Ordering::Relaxed));
        }
        true
    }

    fn buffer(&self) -> &ScopeBuffer {
        &self.shared.scope
    }
}

/// Keeps a [`Scope`] copying audio while it lives (see [`Scope::watch`]).
pub struct ScopeWatch {
    shared: Arc<Shared>,
}

impl Drop for ScopeWatch {
    fn drop(&mut self) {
        self.shared.scope.watchers.fetch_sub(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope() -> Scope {
        Scope::new(Arc::new(Shared::new(1.0)))
    }

    #[test]
    fn copies_only_while_watched() {
        let scope = scope();
        scope.buffer().push(&[0.5; 64], 2, 48_000);
        assert!(!scope.latest(&mut [0.0; 8]));
        let first = scope.watch();
        let second = scope.clone().watch();
        scope.buffer().push(&[0.5; 64], 2, 48_000);
        assert!(scope.latest(&mut [0.0; 8]));
        assert_eq!(scope.sample_rate(), 48_000);
        // One visualiser stopping doesn't stop the copy for the other.
        drop(first);
        scope.buffer().push(&[0.25; 64], 2, 48_000);
        let mut out = [0.0; 2];
        assert!(scope.latest(&mut out));
        assert_eq!(out, [0.25, 0.25]);
        drop(second);
        assert_eq!(scope.buffer().watchers.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn mixes_channels_and_returns_the_newest_frames() {
        let scope = scope();
        let _watch = scope.watch();
        let block: Vec<f32> = (0..100)
            .flat_map(|frame| [frame as f32, frame as f32 + 2.0])
            .collect();
        scope.buffer().push(&block, 2, 44_100);
        let mut out = [0.0; 4];
        assert!(scope.latest(&mut out));
        assert_eq!(out, [97.0, 98.0, 99.0, 100.0]);
    }

    #[test]
    fn pads_with_silence_before_the_first_audio() {
        let scope = scope();
        let _watch = scope.watch();
        scope.buffer().push(&[0.25; 3], 1, 44_100);
        let mut out = [9.0; 5];
        assert!(scope.latest(&mut out));
        assert_eq!(out, [0.0, 0.0, 0.25, 0.25, 0.25]);
    }

    #[test]
    fn latency_holds_back_audio_not_yet_heard() {
        let scope = scope();
        let _watch = scope.watch();
        scope.shared.set_latency(Duration::from_millis(100));
        // 0.2 s at 1 kHz: the first half is being heard, the second is still on its way.
        let block: Vec<f32> = (0..200).map(|frame| frame as f32).collect();
        scope.buffer().push(&block, 1, 1_000);
        let mut out = [0.0; 2];
        assert!(scope.latest(&mut out));
        assert!(out[1] < 105.0 && out[1] >= 99.0, "{out:?}");
    }

    #[test]
    fn stale_audio_is_not_shown() {
        let scope = scope();
        let _watch = scope.watch();
        scope.buffer().push(&[0.5; 8], 1, 48_000);
        scope.buffer().written_at_ns.store(0, Ordering::Relaxed);
        std::thread::sleep(STALE_AFTER + Duration::from_millis(20));
        assert!(!scope.latest(&mut [0.0; 4]));
    }
}
