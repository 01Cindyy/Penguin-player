//! The player's internal operations. Public entry points live in `mod.rs`.

use std::time::Duration;

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use mp_core::{PlayQueue, PlayerEvent, RepeatMode, TrackId};

use super::imp::PendingLoad;
use super::{PlaybackBackend, Player, PlayerState};
use crate::model::{LibraryStore, TrackObject};

/// Pressing "previous" later than this into a track restarts it instead.
const RESTART_THRESHOLD: Duration = Duration::from_secs(3);

/// How often `position` is refreshed while playing.
const POSITION_INTERVAL: Duration = Duration::from_millis(250);

/// After this many failures in a row, playback stops instead of trying the next
/// track, so an unplugged drive doesn't make the player work through the whole queue.
pub(super) const MAX_CONSECUTIVE_FAILURES: usize = 25;

impl Player {
    pub(super) fn store(&self) -> &LibraryStore {
        self.imp()
            .store
            .get()
            .expect("the store is set when the player is created")
    }

    /// Runs `f` with the engine, if the engine started.
    pub(super) fn with_backend(&self, f: impl FnOnce(&dyn PlaybackBackend)) {
        if let Some(backend) = self.imp().backend.borrow().as_deref() {
            f(backend);
        }
    }

    fn has_backend(&self) -> bool {
        self.imp().backend.borrow().is_some()
    }

    pub(super) fn resume(&self) {
        match self.state() {
            PlayerState::Playing => {}
            PlayerState::Paused => self.with_backend(|backend| backend.play()),
            PlayerState::Stopped => {
                // Pressing play is a fresh attempt, also for tracks that failed before.
                self.forget_failures();
                let current = self.imp().queue.borrow().current();
                match current {
                    Some(track) => self.load(track, true),
                    None => self.play_all(self.store().track_ids()),
                }
            }
        }
    }

    pub(super) fn start_queue(&self, tracks: Vec<TrackId>, start: Option<usize>) {
        if tracks.is_empty() {
            return;
        }
        self.forget_failures();
        let first = self.imp().queue.borrow_mut().set_tracks(tracks, start);
        if let Some(track) = first {
            self.load(track, true);
        }
        self.emit_queue_changed();
    }

    pub(super) fn insert_tracks(
        &self,
        tracks: &[TrackId],
        insert: impl FnOnce(&mut PlayQueue, &[TrackId]),
    ) {
        if tracks.is_empty() {
            return;
        }
        let was_empty = self.imp().queue.borrow().is_empty();
        insert(&mut self.imp().queue.borrow_mut(), tracks);
        let current = self.imp().queue.borrow().current();
        match current {
            // Show the new current track paused, ready for the play button.
            Some(track) if was_empty => self.load(track, false),
            _ => self.sync_next(),
        }
        self.emit_queue_changed();
    }

    /// The user pressed "next": move on even when repeating one track.
    pub(super) fn skip_next(&self) {
        // Moving by hand is a fresh attempt, also for tracks that failed before.
        self.forget_failures();
        self.move_on();
    }

    /// Loads the next playable track, even when repeating one, or stops at the end.
    pub(super) fn move_on(&self) {
        let autoplay = self.state() != PlayerState::Paused;
        match self.move_to_next_playable(true) {
            Some(track) => self.load(track, autoplay),
            None => self.stop_playback(),
        }
        self.emit_queue_changed();
    }

    pub(super) fn skip_previous(&self) {
        let elapsed = Duration::from_millis(self.position());
        if elapsed > RESTART_THRESHOLD && self.current_track().is_some() {
            self.seek(Duration::ZERO);
            return;
        }
        self.forget_failures();
        let previous = self.imp().queue.borrow_mut().skip_previous();
        if let Some(track) = previous {
            self.load(track, self.state() != PlayerState::Paused);
            self.emit_queue_changed();
        }
    }

    pub(super) fn stop_playback(&self) {
        self.with_backend(|backend| backend.stop());
        self.set_position(0);
        self.set_state(PlayerState::Stopped);
    }

    pub(super) fn seek(&self, position: Duration) {
        if self.current_track().is_none() {
            return;
        }
        let duration = Duration::from_millis(self.duration());
        let position = if duration.is_zero() {
            position
        } else {
            position.min(duration)
        };
        self.with_backend(|backend| backend.seek(position));
        self.set_position(position.as_millis() as u64);
    }

    pub(super) fn seek_by(&self, delta_ms: i64) {
        let target = self.position().saturating_add_signed(delta_ms);
        self.seek(Duration::from_millis(target));
    }

    /// Opens `track` in the engine and publishes it as the current track. A track that
    /// has disappeared from the library is treated like one that failed to open.
    pub(super) fn load(&self, track: TrackId, autoplay: bool) {
        let pending = PendingLoad { track, autoplay };
        let Some(object) = self.store().track(track) else {
            self.imp().pending_load.set(Some(pending));
            self.on_track_failed(track, "the file is no longer in the library");
            return;
        };
        if self.has_backend() {
            self.imp().pending_load.set(Some(pending));
        }
        let path = object.track().path.clone();
        self.with_backend(|backend| backend.load(track, path, autoplay));
        self.set_current_track(Some(&object));
        self.set_duration(object.track().duration_ms);
        self.set_position(0);
        self.sync_next();
    }

    /// Tells the engine which track follows the current one, for gapless playback.
    /// Must run after every change that can affect what plays next.
    pub(super) fn sync_next(&self) {
        let next = self
            .next_playable_position(false)
            .and_then(|position| self.imp().queue.borrow().get(position))
            .and_then(|id| self.store().track(id))
            .map(|object| (object.track_id(), object.track().path.clone()));
        self.with_backend(|backend| backend.set_next(next));
    }

    /// Makes the next playable track current (see
    /// [`PlayQueue::following_positions`]) and returns it, or `None` if playback
    /// should stop.
    pub(super) fn move_to_next_playable(&self, skipping: bool) -> Option<TrackId> {
        let position = self.next_playable_position(skipping)?;
        self.imp().queue.borrow_mut().jump_to(position)
    }

    /// Where playback goes after the current track, passing over tracks that failed
    /// or are no longer in the library. `None` at the end of the queue, or once too
    /// many tracks failed in a row.
    fn next_playable_position(&self, skipping: bool) -> Option<usize> {
        let imp = self.imp();
        if imp.failure_streak.get() >= MAX_CONSECUTIVE_FAILURES {
            return None;
        }
        let queue = imp.queue.borrow();
        let unplayable = imp.unplayable.borrow();
        let store = self.store();
        queue.following_positions(skipping).find(|&position| {
            queue
                .get(position)
                .is_some_and(|track| !unplayable.contains(&track) && store.track(track).is_some())
        })
    }

    /// Clears the record of failed tracks, so every track gets another chance.
    pub(super) fn forget_failures(&self) {
        self.imp().failure_streak.set(0);
        self.imp().unplayable.borrow_mut().clear();
    }

    /// Sets the volume level. With `unmute`, a level above zero also unmutes: someone
    /// moving a volume control while muted wants to hear it again.
    pub(super) fn apply_volume(&self, volume: f64, unmute: bool) {
        if !volume.is_finite() {
            return;
        }
        let volume = volume.clamp(0.0, 1.0);
        let imp = self.imp();
        let volume_changed = imp.volume.replace(volume) != volume;
        let unmuted = unmute && volume > 0.0 && imp.muted.replace(false);
        if !volume_changed && !unmuted {
            return;
        }
        self.apply_output_volume();
        if volume_changed {
            self.notify_volume();
        }
        if unmuted {
            self.notify_muted();
        }
    }

    pub(super) fn apply_muted(&self, muted: bool) {
        if self.imp().muted.replace(muted) == muted {
            return;
        }
        self.apply_output_volume();
        self.notify_muted();
    }

    fn apply_output_volume(&self) {
        let volume = self.effective_volume();
        self.with_backend(|backend| backend.set_volume(volume));
        self.broadcast(self.volume_event());
    }

    pub(super) fn volume_event(&self) -> PlayerEvent {
        PlayerEvent::VolumeChanged {
            volume: self.volume() as f32,
            muted: self.muted(),
        }
    }

    pub(super) fn apply_shuffle(&self, enabled: bool) {
        if self.imp().shuffle.replace(enabled) == enabled {
            return;
        }
        self.imp().queue.borrow_mut().set_shuffle(enabled);
        self.sync_next();
        self.notify_shuffle();
        self.emit_queue_changed();
        self.broadcast(PlayerEvent::ShuffleChanged(enabled));
    }

    pub(super) fn apply_repeat(&self, mode: RepeatMode) {
        if self.imp().repeat.replace(mode.into()) == mode.into() {
            return;
        }
        self.imp().queue.borrow_mut().set_repeat(mode);
        self.sync_next();
        self.notify_repeat();
        self.broadcast(PlayerEvent::RepeatChanged(mode));
    }

    /// Publishes the current track and moves the `playing` flag to it. A new object
    /// for the same track (its details changed in a rescan) is published to the UI
    /// only: plugins and media controls hear of changes of track.
    pub(super) fn set_current_track(&self, track: Option<&TrackObject>) {
        let old = self.imp().current_track.replace(track.cloned());
        if old.as_ref() == track {
            return;
        }
        if let Some(old) = &old {
            old.mark_playing(false);
        }
        if let Some(track) = track {
            track.mark_playing(true);
        }
        self.notify_current_track();
        let same_track = old.map(|old| old.track_id()) == track.map(TrackObject::track_id);
        if !same_track {
            let event_track = track.map(|object| Box::new(object.track().clone()));
            self.broadcast(PlayerEvent::TrackChanged(event_track));
        }
    }

    /// After a library refresh, swaps in the new object for the current track.
    pub(super) fn refresh_current_track(&self) {
        let Some(current) = self.current_track() else {
            return;
        };
        match self.store().track(current.track_id()) {
            Some(fresh) if fresh != current => self.set_current_track(Some(&fresh)),
            _ => {}
        }
    }

    pub(super) fn set_state(&self, state: PlayerState) {
        if self.imp().state.replace(state) == state {
            return;
        }
        self.notify_state();
        self.update_position_timer();
        self.broadcast(PlayerEvent::StateChanged(state.into()));
    }

    pub(super) fn set_position(&self, position_ms: u64) {
        if self.imp().position.replace(position_ms) != position_ms {
            self.notify_position();
        }
    }

    pub(super) fn set_duration(&self, duration_ms: u64) {
        if self.imp().duration.replace(duration_ms) != duration_ms {
            self.notify_duration();
        }
    }

    /// Polls the engine position only while playing, so an idle player costs nothing.
    fn update_position_timer(&self) {
        let playing = self.state() == PlayerState::Playing;
        let running = self.imp().position_timer.borrow().is_some();
        if playing && !running {
            let weak = self.downgrade();
            let timer = glib::timeout_add_local(POSITION_INTERVAL, move || {
                let Some(player) = weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                player.poll_position();
                glib::ControlFlow::Continue
            });
            self.imp().position_timer.replace(Some(timer));
        } else if !playing && running {
            if let Some(timer) = self.imp().position_timer.take() {
                timer.remove();
            }
            self.poll_position();
        }
    }

    fn poll_position(&self) {
        let mut position = None;
        self.with_backend(|backend| position = Some(backend.position()));
        if let Some(position) = position {
            self.set_position(position.as_millis() as u64);
        }
    }

    pub(super) fn report_error(&self, message: &str) {
        self.emit_by_name::<()>("playback-error", &[&message]);
    }

    pub(super) fn emit_queue_changed(&self) {
        self.emit_by_name::<()>("queue-changed", &[]);
    }
}
