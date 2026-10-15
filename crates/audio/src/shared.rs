//! State shared by the engine handle, the engine thread and the output callback.
//!
//! Everything here is a plain atomic (the scope too) so the real-time output callback
//! never waits on a lock, and so `AudioEngine::position()` and friends are a few loads on
//! the UI thread.

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use mp_core::PlaybackState;

use crate::scope::ScopeBuffer;

pub(crate) struct Shared {
    /// Linear volume as `f32` bits.
    volume: AtomicU32,
    /// Whether the output should be playing audio. It fades out when this turns false.
    playing: AtomicBool,
    state: AtomicU8,

    // Written by the output callback.
    /// Serial of the segment the output is currently playing.
    segment: AtomicU64,
    /// Track position of the audio last handed to the device, in nanoseconds.
    position_ns: AtomicU64,
    /// The output has faded out and only produces silence.
    silent: AtomicBool,
    /// How long audio takes from the callback to the speakers, in nanoseconds.
    latency_ns: AtomicU64,

    // Written by the engine when playback jumps (load, seek, stop). Until the output reaches
    // segment `jump_serial`, the position reported is the jump target, so the UI moves at once.
    jump_serial: AtomicU64,
    jump_position_ns: AtomicU64,

    /// A copy of the audio played, written by the output callback.
    pub scope: ScopeBuffer,
}

impl Shared {
    pub fn new(volume: f32) -> Self {
        let shared = Self {
            volume: AtomicU32::new(0),
            playing: AtomicBool::new(false),
            state: AtomicU8::new(encode_state(PlaybackState::Stopped)),
            segment: AtomicU64::new(0),
            position_ns: AtomicU64::new(0),
            silent: AtomicBool::new(true),
            latency_ns: AtomicU64::new(0),
            jump_serial: AtomicU64::new(0),
            jump_position_ns: AtomicU64::new(0),
            scope: ScopeBuffer::new(),
        };
        shared.set_volume(volume);
        shared
    }

    pub fn volume(&self) -> f32 {
        f32::from_bits(self.volume.load(Ordering::Relaxed))
    }

    /// Stores `volume` clamped to `0.0..=1.0`. NaN is ignored.
    pub fn set_volume(&self, volume: f32) {
        if !volume.is_nan() {
            self.volume
                .store(volume.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        }
    }

    pub fn is_playing(&self) -> bool {
        self.playing.load(Ordering::Acquire)
    }

    pub fn set_playing(&self, playing: bool) {
        if playing {
            self.silent.store(false, Ordering::Release);
        }
        self.playing.store(playing, Ordering::Release);
    }

    pub fn state(&self) -> PlaybackState {
        decode_state(self.state.load(Ordering::Acquire))
    }

    pub fn set_state(&self, state: PlaybackState) {
        self.state.store(encode_state(state), Ordering::Release);
    }

    pub fn segment(&self) -> u64 {
        self.segment.load(Ordering::Acquire)
    }

    pub fn is_silent(&self) -> bool {
        self.silent.load(Ordering::Acquire)
    }

    pub fn latency(&self) -> Duration {
        Duration::from_nanos(self.latency_ns.load(Ordering::Relaxed))
    }

    pub fn set_latency(&self, latency: Duration) {
        self.latency_ns
            .store(duration_to_nanos(latency), Ordering::Relaxed);
    }

    /// Called by the output callback after each block of audio.
    pub fn publish_progress(&self, position_ns: u64, silent: bool) {
        self.position_ns.store(position_ns, Ordering::Relaxed);
        self.silent.store(silent, Ordering::Release);
    }

    /// Called by the output callback when it starts playing a new segment.
    pub fn publish_segment(&self, serial: u64, position_ns: u64) {
        self.position_ns.store(position_ns, Ordering::Relaxed);
        self.segment.store(serial, Ordering::Release);
    }

    /// Makes `position()` report `position` until the output reaches segment `serial`.
    pub fn jump(&self, serial: u64, position: Duration) {
        self.jump_position_ns
            .store(duration_to_nanos(position), Ordering::Relaxed);
        self.jump_serial.store(serial, Ordering::Release);
    }

    pub fn position(&self) -> Duration {
        let jump_serial = self.jump_serial.load(Ordering::Acquire);
        let nanos = if self.segment() < jump_serial {
            self.jump_position_ns.load(Ordering::Relaxed)
        } else {
            self.position_ns.load(Ordering::Relaxed)
        };
        Duration::from_nanos(nanos)
    }
}

pub(crate) fn duration_to_nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

/// How long `frames` take to play at `sample_rate`, in nanoseconds.
pub(crate) fn frames_to_nanos(frames: u64, sample_rate: u64) -> u64 {
    let nanos = u128::from(frames) * 1_000_000_000 / u128::from(sample_rate.max(1));
    u64::try_from(nanos).unwrap_or(u64::MAX)
}

fn encode_state(state: PlaybackState) -> u8 {
    match state {
        PlaybackState::Stopped => 0,
        PlaybackState::Playing => 1,
        PlaybackState::Paused => 2,
    }
}

fn decode_state(value: u8) -> PlaybackState {
    match value {
        1 => PlaybackState::Playing,
        2 => PlaybackState::Paused,
        _ => PlaybackState::Stopped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_to_nanos_is_exact_for_long_sessions() {
        // The null output counts every frame of a session, at up to 768 kHz.
        assert_eq!(
            frames_to_nanos(768_000 * 86_400, 768_000),
            86_400 * 1_000_000_000
        );
        assert_eq!(frames_to_nanos(1, 44_100), 22_675);
    }
}
