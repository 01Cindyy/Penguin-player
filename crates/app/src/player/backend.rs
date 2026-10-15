use std::path::PathBuf;
use std::time::Duration;

use mp_audio::AudioEngine;
use mp_core::TrackId;

/// The engine operations the [`super::Player`] relies on.
///
/// [`AudioEngine`] is the real implementation; tests substitute a recording fake so the
/// queue logic can be exercised without decoding files or opening an audio device.
pub trait PlaybackBackend {
    fn load(&self, track: TrackId, path: PathBuf, autoplay: bool);
    fn set_next(&self, next: Option<(TrackId, PathBuf)>);
    fn play(&self);
    fn pause(&self);
    fn stop(&self);
    fn seek(&self, position: Duration);
    fn set_volume(&self, volume: f32);
    fn set_gapless(&self, enabled: bool);
    fn position(&self) -> Duration;
}

impl PlaybackBackend for AudioEngine {
    fn load(&self, track: TrackId, path: PathBuf, autoplay: bool) {
        AudioEngine::load(self, track, path, autoplay);
    }

    fn set_next(&self, next: Option<(TrackId, PathBuf)>) {
        AudioEngine::set_next(self, next);
    }

    fn play(&self) {
        AudioEngine::play(self);
    }

    fn pause(&self) {
        AudioEngine::pause(self);
    }

    fn stop(&self) {
        AudioEngine::stop(self);
    }

    fn seek(&self, position: Duration) {
        AudioEngine::seek(self, position);
    }

    fn set_volume(&self, volume: f32) {
        AudioEngine::set_volume(self, volume);
    }

    fn set_gapless(&self, enabled: bool) {
        AudioEngine::set_gapless(self, enabled);
    }

    fn position(&self) -> Duration {
        AudioEngine::position(self)
    }
}
