//! Reacting to what the audio engine reports.
//!
//! Engine events arrive asynchronously, so some describe a state the player has
//! already left: after `load(b)`, events about the previous track can still arrive
//! before `b`'s own `TrackStarted` or `TrackFailed`. Those are recognised through
//! `pending_load` and ignored.

use std::time::Duration;

use gtk::subclass::prelude::*;
use mp_audio::EngineEvent;
use mp_core::{PlayerEvent, TrackId};

use super::playback::MAX_CONSECUTIVE_FAILURES;
use super::{Player, PlayerState};

impl Player {
    pub(super) fn handle_engine_event(&self, event: EngineEvent) {
        log::debug!("Engine event: {event:?}");
        match event {
            EngineEvent::TrackStarted {
                track,
                duration,
                from_queue,
            } => self.on_track_started(track, duration, from_queue),
            EngineEvent::StateChanged(state) => self.set_state(state.into()),
            EngineEvent::EndOfQueue => self.on_end_of_queue(),
            EngineEvent::Seeked(position) => {
                self.set_position(position.as_millis() as u64);
                self.broadcast(PlayerEvent::Seeked { position });
            }
            EngineEvent::TrackFailed { track, message } => self.on_track_failed(track, &message),
            // The engine reports the state it falls back to (paused where it was, if it
            // has a track) with its own `StateChanged`.
            EngineEvent::OutputError(message) => {
                log::error!("Audio output error: {message}");
                self.report_error(&format!("Audio output failed: {message}"));
            }
        }
    }

    /// The engine opened a track: either the one the player loaded (`from_queue` is
    /// false, also when not autoplaying) or the preloaded next one in a gapless
    /// transition. Either way it is now the current track.
    fn on_track_started(&self, track: TrackId, duration: Option<Duration>, from_queue: bool) {
        let imp = self.imp();
        match imp.pending_load.get() {
            Some(pending) if pending.track == track && !from_queue => imp.pending_load.set(None),
            Some(_) => return,
            None => {}
        }
        let Some(queue_moved) = self.follow_engine(track, from_queue) else {
            // The queue was cleared or replaced since.
            return;
        };
        imp.failure_streak.set(0);
        let Some(object) = self.store().track(track) else {
            // The library dropped it after the engine had preloaded it.
            self.move_on();
            return;
        };
        self.set_current_track(Some(&object));
        let fallback = object.track().duration_ms;
        self.set_duration(duration.map_or(fallback, |d| d.as_millis() as u64));
        self.set_position(0);
        self.sync_next();
        if queue_moved {
            self.emit_queue_changed();
        }
    }

    /// Moves the queue to `track`, which the engine just started. Returns whether the
    /// position changed, or `None` if the track is no longer queued.
    fn follow_engine(&self, track: TrackId, from_queue: bool) -> Option<bool> {
        let mut queue = self.imp().queue.borrow_mut();
        if !from_queue && queue.current() == Some(track) {
            return Some(false);
        }
        let before = queue.position();
        let position = queue
            .following_positions(false)
            .find(|&position| queue.get(position) == Some(track))
            .or_else(|| queue.tracks().iter().position(|&id| id == track))?;
        queue.jump_to(position);
        Some(queue.position() != before)
    }

    /// The current track ended with nothing queued in time (gapless off, a very
    /// short track, or the preloaded track failed), so the player moves on itself.
    fn on_end_of_queue(&self) {
        if self.imp().pending_load.get().is_some() {
            return;
        }
        match self.move_to_next_playable(false) {
            Some(track) => self.load(track, true),
            None => {
                self.set_state(PlayerState::Stopped);
                self.set_position(0);
            }
        }
        self.emit_queue_changed();
    }

    /// A track couldn't be opened or decoded. It is skipped from now on; only the
    /// first failure in a row is reported.
    ///
    /// If it was the track just loaded, the player loads the next playable one. Any
    /// other failure (the preloaded next track, or the current one breaking off) only
    /// changes what is preloaded: the engine moves on by itself, with the preloaded
    /// track or `EndOfQueue`, so advancing here as well would skip a track.
    pub(super) fn on_track_failed(&self, track: TrackId, message: &str) {
        log::warn!("Could not play track {}: {message}", track.0);
        let imp = self.imp();
        imp.unplayable.borrow_mut().insert(track);
        let streak = imp.failure_streak.get() + 1;
        imp.failure_streak.set(streak);
        if streak == 1 {
            let title = self
                .store()
                .track(track)
                .map_or_else(|| "a song".to_owned(), |t| format!("“{}”", t.title()));
            self.report_error(&format!("Couldn’t play {title}: {message}"));
        }

        match imp.pending_load.get() {
            Some(pending) if pending.track == track => {
                imp.pending_load.set(None);
                self.move_past_failed_load(pending.autoplay, streak);
            }
            Some(_) => {}
            None => self.sync_next(),
        }
    }

    fn move_past_failed_load(&self, autoplay: bool, streak: usize) {
        match self.move_to_next_playable(true) {
            Some(next) => self.load(next, autoplay),
            None => {
                self.stop_playback();
                if streak >= MAX_CONSECUTIVE_FAILURES {
                    self.report_error("Playback stopped: too many songs couldn’t be played");
                } else if streak > 1 {
                    self.report_error("Playback stopped: none of the next songs could be played");
                }
            }
        }
        self.emit_queue_changed();
    }
}
