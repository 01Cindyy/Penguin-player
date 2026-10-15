//! Playback: the [`Player`] GObject drives the audio engine and the play queue.
//!
//! Every way of controlling playback (buttons, shortcuts, media keys, plugins) ends up
//! in [`Player::execute`] or one of the typed helpers next to it, and every change is
//! published as GObject properties for the UI and as [`PlayerEvent`]s for plugins and
//! OS media controls.

mod backend;
mod engine_events;
mod imp;
mod playback;
mod types;

#[cfg(test)]
mod tests;

use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use mp_audio::{AudioEngine, EngineConfig, EngineEvent};
use mp_core::{PlaybackState, PlayerCommand, PlayerEvent, RepeatMode, Settings, TrackId};

pub use backend::PlaybackBackend;
pub use types::{PlayerRepeat, PlayerState};

use crate::model::LibraryStore;

/// Setting this environment variable to `1` decodes in real time but discards the
/// audio, so the app can run without making sound (tests, screenshots, CI).
pub const NULL_AUDIO_ENV: &str = "PENGUIN_PLAYER_NULL_AUDIO";

/// How much the volume shortcuts change the volume.
pub const VOLUME_STEP: f64 = 0.05;

glib::wrapper! {
    /// Owns the audio engine and the play queue, and publishes the playback state.
    ///
    /// Properties: `state`, `current-track`, `position` and `duration` (milliseconds),
    /// `volume`, `muted`, `shuffle`, `repeat`. Writable properties are applied exactly
    /// like the equivalent [`PlayerCommand`], so they can be bound to widgets directly.
    pub struct Player(ObjectSubclass<imp::Player>);
}

impl Player {
    /// Creates the player and starts the audio engine. If the engine can't start the
    /// player still works (queue, UI) and [`Self::startup_error`] explains why.
    pub fn new(store: &LibraryStore, settings: &Settings) -> Self {
        let config = EngineConfig {
            volume: if settings.muted { 0.0 } else { settings.volume },
            gapless: settings.gapless,
            null_output: null_audio_requested(),
        };
        match AudioEngine::new(config) {
            Ok((engine, events)) => {
                let scope = engine.scope();
                let player = Self::from_backend(store, settings, Some(Box::new(engine)));
                assert!(
                    player.imp().scope.set(scope).is_ok(),
                    "a new player has no scope"
                );
                player.listen_to_engine(events);
                player
            }
            Err(err) => {
                log::error!("Audio engine unavailable: {err}");
                let player = Self::from_backend(store, settings, None);
                player.imp().startup_error.replace(Some(err.to_string()));
                player
            }
        }
    }

    fn from_backend(
        store: &LibraryStore,
        settings: &Settings,
        backend: Option<Box<dyn PlaybackBackend>>,
    ) -> Self {
        let player: Self = glib::Object::new();
        let imp = player.imp();
        imp.store
            .set(store.clone())
            .expect("a new player has no store yet");
        imp.backend.replace(backend);
        imp.volume.set(f64::from(settings.volume));
        imp.muted.set(settings.muted);
        imp.shuffle.set(settings.shuffle);
        imp.repeat.set(settings.repeat.into());
        imp.queue
            .replace(mp_core::PlayQueue::new(settings.shuffle, settings.repeat));

        let weak = player.downgrade();
        store.connect_tracks_changed(move |_| {
            if let Some(player) = weak.upgrade() {
                player.refresh_current_track();
                // The preloaded track may just have left the library.
                player.sync_next();
            }
        });
        player
    }

    fn listen_to_engine(&self, events: async_channel::Receiver<EngineEvent>) {
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            while let Ok(event) = events.recv().await {
                let Some(player) = weak.upgrade() else { break };
                player.handle_engine_event(event);
            }
        });
    }

    /// Stops playback and shuts the audio engine down. The player stays usable as an
    /// inert object, so late signals during shutdown are harmless.
    pub fn shutdown(&self) {
        if let Some(timer) = self.imp().position_timer.take() {
            timer.remove();
        }
        drop(self.imp().backend.take());
    }

    /// The audio being played, for a visualiser. `None` if the engine couldn't start.
    pub fn scope(&self) -> Option<mp_audio::Scope> {
        self.imp().scope.get().cloned()
    }

    /// Why the audio engine could not start, if it couldn't.
    pub fn startup_error(&self) -> Option<String> {
        self.imp().startup_error.borrow().clone()
    }

    /// The single entry point for playback commands from any source.
    pub fn execute(&self, command: PlayerCommand) {
        match command {
            PlayerCommand::Play => self.resume(),
            PlayerCommand::Pause => self.with_backend(|backend| backend.pause()),
            PlayerCommand::TogglePlayPause => {
                if self.state() == PlayerState::Playing {
                    self.with_backend(|backend| backend.pause());
                } else {
                    self.resume();
                }
            }
            PlayerCommand::Stop => self.stop_playback(),
            PlayerCommand::Next => self.skip_next(),
            PlayerCommand::Previous => self.skip_previous(),
            PlayerCommand::SeekTo(position) => self.seek(position),
            PlayerCommand::SeekBy(delta_ms) => self.seek_by(delta_ms),
            PlayerCommand::SetVolume { volume, unmute } => {
                self.apply_volume(f64::from(volume), unmute);
            }
            PlayerCommand::SetShuffle(enabled) => self.apply_shuffle(enabled),
            PlayerCommand::SetRepeat(mode) => self.apply_repeat(mode),
            PlayerCommand::PlayTracks { tracks, start } => self.start_queue(tracks, Some(start)),
            PlayerCommand::Raise => self.emit_by_name::<()>("raise-requested", &[]),
            PlayerCommand::Quit => self.emit_by_name::<()>("quit-requested", &[]),
        }
    }

    /// Replaces the queue with `tracks` and plays from index `start`.
    pub fn play_tracks(&self, tracks: Vec<TrackId>, start: usize) {
        self.execute(PlayerCommand::PlayTracks { tracks, start });
    }

    /// Plays `tracks` from the top, or from a random track when shuffle is on.
    pub fn play_all(&self, tracks: Vec<TrackId>) {
        let start = if self.shuffle() { None } else { Some(0) };
        self.start_queue(tracks, start);
    }

    /// Turns shuffle on and plays `tracks` in random order.
    pub fn shuffle_tracks(&self, tracks: Vec<TrackId>) {
        self.apply_shuffle(true);
        self.start_queue(tracks, None);
    }

    /// Inserts `tracks` to play right after the current one.
    pub fn play_next(&self, tracks: &[TrackId]) {
        self.insert_tracks(tracks, |queue, tracks| queue.play_next(tracks));
    }

    /// Appends `tracks` to the end of the queue.
    pub fn enqueue(&self, tracks: &[TrackId]) {
        self.insert_tracks(tracks, |queue, tracks| queue.enqueue(tracks));
    }

    /// Removes the track at play-order `position`. Removing the current track moves on
    /// to the next one.
    pub fn remove_from_queue(&self, position: usize) {
        let was_playing = self.state() == PlayerState::Playing;
        let removed_current = self.imp().queue.borrow_mut().remove(position);
        if removed_current {
            let current = self.imp().queue.borrow().current();
            match current {
                Some(track) => self.load(track, was_playing),
                None => self.clear_queue(),
            }
        }
        self.sync_next();
        self.emit_queue_changed();
    }

    /// Moves a queued track between play-order positions.
    pub fn move_in_queue(&self, from: usize, to: usize) {
        self.imp().queue.borrow_mut().move_track(from, to);
        self.sync_next();
        self.emit_queue_changed();
    }

    /// Plays the track at play-order `position` in the queue.
    pub fn jump_to(&self, position: usize) {
        let track = self.imp().queue.borrow_mut().jump_to(position);
        if let Some(track) = track {
            // Choosing a track by hand retries every track, even those that failed
            // before: the drive they are on may be back.
            self.forget_failures();
            self.load(track, true);
            self.emit_queue_changed();
        }
    }

    /// Stops playback and empties the queue.
    pub fn clear_queue(&self) {
        self.with_backend(|backend| backend.stop());
        self.imp().queue.borrow_mut().clear();
        self.forget_failures();
        self.set_current_track(None);
        self.set_duration(0);
        self.set_position(0);
        self.set_state(PlayerState::Stopped);
        self.sync_next();
        self.emit_queue_changed();
    }

    /// Removes every track after the current one. The current track and the ones
    /// already played stay.
    pub fn clear_upcoming(&self) {
        if self.imp().queue.borrow().upcoming().next().is_none() {
            return;
        }
        self.imp().queue.borrow_mut().clear_upcoming();
        self.sync_next();
        self.emit_queue_changed();
    }

    /// Every queued track in play order.
    pub fn queue_tracks(&self) -> Vec<TrackId> {
        self.imp().queue.borrow().tracks()
    }

    /// Play-order position of the current track.
    pub fn queue_position(&self) -> Option<usize> {
        self.imp().queue.borrow().position()
    }

    pub fn queue_len(&self) -> usize {
        self.imp().queue.borrow().len()
    }

    /// Off → All → One → Off.
    pub fn cycle_repeat(&self) {
        let next = self.repeat_mode().cycle();
        self.execute(PlayerCommand::SetRepeat(next));
    }

    /// Changes the volume by `delta` (see [`VOLUME_STEP`]).
    pub fn change_volume(&self, delta: f64) {
        self.apply_volume(self.volume() + delta, true);
    }

    pub fn set_gapless(&self, enabled: bool) {
        self.with_backend(|backend| backend.set_gapless(enabled));
    }

    pub fn playback_state(&self) -> PlaybackState {
        self.state().into()
    }

    pub fn repeat_mode(&self) -> RepeatMode {
        self.repeat().into()
    }

    /// Volume actually sent to the output: 0 while muted.
    pub fn effective_volume(&self) -> f32 {
        if self.muted() {
            0.0
        } else {
            self.volume() as f32
        }
    }

    /// Events that describe the current state, for listeners that start late
    /// (plugins, media controls): volume, shuffle and repeat, plus the track and the
    /// playback state when a track is loaded.
    pub fn state_events(&self) -> Vec<PlayerEvent> {
        let mut events = vec![
            self.volume_event(),
            PlayerEvent::ShuffleChanged(self.shuffle()),
            PlayerEvent::RepeatChanged(self.repeat_mode()),
        ];
        if let Some(track) = self.current_track() {
            events.push(PlayerEvent::TrackChanged(Some(Box::new(
                track.track().clone(),
            ))));
            events.push(PlayerEvent::StateChanged(self.playback_state()));
        }
        events
    }

    /// Registers a Rust-side listener for every [`PlayerEvent`] (used by plugins and
    /// media controls). Listeners live as long as the player.
    pub fn connect_player_event(&self, listener: impl Fn(&PlayerEvent) + 'static) {
        self.imp().listeners.borrow_mut().push(Rc::new(listener));
    }

    /// Sends `event` to every listener. The player calls this itself for playback
    /// changes; other parts of the app use it for events such as `LibraryUpdated`.
    pub fn broadcast(&self, event: PlayerEvent) {
        let listeners = self.imp().listeners.borrow().clone();
        for listener in listeners {
            listener(&event);
        }
    }

    /// Called whenever the queue's contents, order or position change.
    pub fn connect_queue_changed<F: Fn(&Self) + 'static>(&self, f: F) -> glib::SignalHandlerId {
        self.connect_closure(
            "queue-changed",
            false,
            glib::closure_local!(move |player: &Self| f(player)),
        )
    }

    /// Called with a user-facing message when a track can't be played.
    pub fn connect_playback_error<F: Fn(&Self, &str) + 'static>(
        &self,
        f: F,
    ) -> glib::SignalHandlerId {
        self.connect_closure(
            "playback-error",
            false,
            glib::closure_local!(move |player: &Self, message: &str| f(player, message)),
        )
    }

    /// Called for [`PlayerCommand::Raise`]: the window should come to the front.
    pub fn connect_raise_requested<F: Fn(&Self) + 'static>(&self, f: F) -> glib::SignalHandlerId {
        self.connect_closure(
            "raise-requested",
            false,
            glib::closure_local!(move |player: &Self| f(player)),
        )
    }

    /// Called for [`PlayerCommand::Quit`]: the application should quit.
    pub fn connect_quit_requested<F: Fn(&Self) + 'static>(&self, f: F) -> glib::SignalHandlerId {
        self.connect_closure(
            "quit-requested",
            false,
            glib::closure_local!(move |player: &Self| f(player)),
        )
    }
}

fn null_audio_requested() -> bool {
    std::env::var(NULL_AUDIO_ENV).is_ok_and(|value| value == "1")
}
