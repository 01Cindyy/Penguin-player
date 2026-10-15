//! Gapless audio playback.
//!
//! An engine thread uses Symphonia to decode the current track (and, near its end, the
//! queued next track) into a lock-free ring buffer that a cpal output stream drains.
//!
//! - [`AudioEngine`] is the handle the app holds; it sends commands to the engine thread
//!   and reads position, state and volume from shared atomics.
//! - `engine` runs the engine thread: track slots, transitions and timing.
//! - `decoder`, `mp4` and `opus` turn files into `f32` samples, with gapless trimming.
//! - `convert` and `resample` adapt the audio to the output's channels and sample rate.
//! - `ring` carries audio and track markers to the output callback; `output` holds the
//!   cpal device stream and the null output.
//! - `scope` copies what the output plays for a visualiser, and `spectrum` turns it into
//!   bar heights.
//!
//! Event order: a successful [`AudioEngine::load`] emits `TrackStarted { from_queue: false }`
//! (also when not autoplaying, so the duration is known) followed by any `StateChanged`.
//! [`AudioEngine::set_next`] opens the next file right away, so a broken one is reported
//! with `TrackFailed` while the current track still plays and the app can queue another.

mod convert;
mod decoder;
mod engine;
mod mp4;
#[cfg(feature = "opus")]
mod opus;
mod output;
mod playback;
mod resample;
mod ring;
mod scope;
mod shared;
mod spectrum;

/// The integration tests' fixture writers, shared with the unit tests.
#[cfg(test)]
#[path = "../tests/common/mod.rs"]
mod test_files;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use mp_core::{PlaybackState, TrackId};

use engine::{Command, Engine};
use shared::Shared;

pub use decoder::{DecodeError, StreamInfo, TrackDecoder};
pub use scope::{Scope, ScopeWatch};
pub use spectrum::Spectrum;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EngineConfig {
    /// Linear volume in `0.0..=1.0`.
    pub volume: f32,
    /// Start the queued next track with no gap when the current one ends.
    pub gapless: bool,
    /// Decode in real time but discard the audio instead of opening an output device.
    /// For tests, CI and running the app without making sound.
    pub null_output: bool,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            volume: 1.0,
            gapless: true,
            null_output: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum EngineEvent {
    /// A track started producing sound. `from_queue` is true when it began because the
    /// previous track ended and it was the one given to [`AudioEngine::set_next`].
    TrackStarted {
        track: TrackId,
        duration: Option<Duration>,
        from_queue: bool,
    },
    StateChanged(PlaybackState),
    /// The current track ended with nothing queued, so the engine stopped.
    EndOfQueue,
    /// A seek finished; playback continues from `position`.
    Seeked(Duration),
    /// A track could not be opened or decoded. The engine continues with the queued
    /// next track if there is one, otherwise it stops.
    TrackFailed {
        track: TrackId,
        message: String,
    },
    /// The output device failed and could not be reopened.
    OutputError(String),
}

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("no audio output device is available")]
    NoDevice,
    #[error("audio output error: {0}")]
    Output(String),
    #[error("could not start the audio engine: {0}")]
    Startup(String),
}

/// Handle to the playback engine. Cheap to call from the UI thread: every method only
/// sends a message or reads an atomic. Dropping it stops playback and joins its threads.
pub struct AudioEngine {
    commands: Sender<Command>,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl AudioEngine {
    /// Starts the engine. The output device is opened lazily on first playback.
    pub fn new(
        config: EngineConfig,
    ) -> Result<(Self, async_channel::Receiver<EngineEvent>), AudioError> {
        let (commands, command_rx) = mpsc::channel();
        let (events, event_rx) = async_channel::unbounded();
        let shared = Arc::new(Shared::new(config.volume));
        let engine = Engine::new(
            config,
            command_rx,
            commands.clone(),
            events,
            Arc::clone(&shared),
        );
        let thread = thread::Builder::new()
            .name("mp-audio-engine".into())
            .spawn(move || engine.run())
            .map_err(|error| AudioError::Startup(error.to_string()))?;
        let engine = Self {
            commands,
            shared,
            thread: Some(thread),
        };
        Ok((engine, event_rx))
    }

    /// Replaces whatever is playing with `path` and clears the queued next track.
    /// With `autoplay` false the track is loaded paused at the start.
    pub fn load(&self, track: TrackId, path: PathBuf, autoplay: bool) {
        self.send(Command::Load {
            track,
            path,
            autoplay,
        });
    }

    /// Sets (or clears) the track to continue with when the current one ends.
    /// May be called at any time; the latest value wins.
    pub fn set_next(&self, next: Option<(TrackId, PathBuf)>) {
        self.send(Command::SetNext(next));
    }

    pub fn play(&self) {
        self.send(Command::Play);
    }

    pub fn pause(&self) {
        self.send(Command::Pause);
    }

    /// Stops playback and unloads the current and next tracks.
    pub fn stop(&self) {
        self.send(Command::Stop);
    }

    pub fn seek(&self, position: Duration) {
        self.send(Command::Seek(position));
    }

    /// Linear volume in `0.0..=1.0`; applied without interrupting playback.
    pub fn set_volume(&self, volume: f32) {
        self.shared.set_volume(volume);
    }

    pub fn set_gapless(&self, enabled: bool) {
        self.send(Command::SetGapless(enabled));
    }

    /// Position within the current track of the audio actually being heard.
    pub fn position(&self) -> Duration {
        self.shared.position()
    }

    pub fn state(&self) -> PlaybackState {
        self.shared.state()
    }

    /// The audio being played, for a visualiser.
    pub fn scope(&self) -> Scope {
        Scope::new(Arc::clone(&self.shared))
    }

    fn send(&self, command: Command) {
        // Fails only if the engine thread has exited, which only happens on drop.
        let _ = self.commands.send(command);
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        self.send(Command::Shutdown);
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            log::error!("the audio engine thread panicked");
        }
    }
}
