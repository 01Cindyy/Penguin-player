use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use mp_audio::EngineEvent;
use mp_core::{PlaybackState, PlayerCommand, PlayerEvent, RepeatMode, Settings, TrackId};

use super::*;
use crate::model::{LibraryStore, test_track};

#[derive(Debug, Clone, PartialEq)]
enum Call {
    Load(i64, bool),
    SetNext(Option<i64>),
    Play,
    Pause,
    Stop,
    Seek(Duration),
    Volume(f32),
}

#[derive(Default)]
struct FakeBackend {
    calls: Rc<RefCell<Vec<Call>>>,
    position: Rc<Cell<Duration>>,
}

impl PlaybackBackend for FakeBackend {
    fn load(&self, track: TrackId, _path: PathBuf, autoplay: bool) {
        self.calls.borrow_mut().push(Call::Load(track.0, autoplay));
    }
    fn set_next(&self, next: Option<(TrackId, PathBuf)>) {
        self.calls
            .borrow_mut()
            .push(Call::SetNext(next.map(|(id, _)| id.0)));
    }
    fn play(&self) {
        self.calls.borrow_mut().push(Call::Play);
    }
    fn pause(&self) {
        self.calls.borrow_mut().push(Call::Pause);
    }
    fn stop(&self) {
        self.calls.borrow_mut().push(Call::Stop);
    }
    fn seek(&self, position: Duration) {
        self.calls.borrow_mut().push(Call::Seek(position));
    }
    fn set_volume(&self, volume: f32) {
        self.calls.borrow_mut().push(Call::Volume(volume));
    }
    fn set_gapless(&self, _enabled: bool) {}
    fn position(&self) -> Duration {
        self.position.get()
    }
}

/// Starting playback adds a timer to the default main context, which only one thread
/// may touch at a time, so player tests run one after another.
static MAIN_CONTEXT_LOCK: Mutex<()> = Mutex::new(());

struct Harness {
    player: Player,
    calls: Rc<RefCell<Vec<Call>>>,
    errors: Rc<RefCell<Vec<String>>>,
    events: Rc<RefCell<Vec<PlayerEvent>>>,
    _lock: MutexGuard<'static, ()>,
}

impl Harness {
    fn new(track_count: i64) -> Self {
        Self::with_settings(track_count, Settings::default())
    }

    fn with_settings(track_count: i64, settings: Settings) -> Self {
        let lock = MAIN_CONTEXT_LOCK
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let store = LibraryStore::new();
        store.replace_tracks((1..=track_count).map(library_track).collect());
        let backend = FakeBackend::default();
        let calls = backend.calls.clone();
        let player = Player::from_backend(&store, &settings, Some(Box::new(backend)));

        let errors = Rc::new(RefCell::new(Vec::new()));
        let sink = errors.clone();
        player.connect_playback_error(move |_, message| sink.borrow_mut().push(message.to_owned()));
        let events = Rc::new(RefCell::new(Vec::new()));
        let sink = events.clone();
        player.connect_player_event(move |event| sink.borrow_mut().push(event.clone()));

        Self {
            player,
            calls,
            errors,
            events,
            _lock: lock,
        }
    }

    /// A rescan after which only the tracks `ids` are left in the library.
    fn keep_in_library(&self, ids: &[i64]) {
        let tracks = ids.iter().copied().map(library_track).collect();
        self.player.store().replace_tracks(tracks);
    }

    fn take_calls(&self) -> Vec<Call> {
        self.calls.take()
    }

    fn current(&self) -> Option<i64> {
        self.player.current_track().map(|t| t.id())
    }

    fn ids(ids: &[i64]) -> Vec<TrackId> {
        ids.iter().copied().map(TrackId).collect()
    }

    /// What the engine reports when it opened a track.
    fn started(&self, id: i64, from_queue: bool) {
        self.player.handle_engine_event(EngineEvent::TrackStarted {
            track: TrackId(id),
            duration: Some(Duration::from_secs(10)),
            from_queue,
        });
    }

    fn failed(&self, id: i64) {
        self.player.handle_engine_event(EngineEvent::TrackFailed {
            track: TrackId(id),
            message: "corrupt".into(),
        });
    }

    fn loads(&self) -> Vec<i64> {
        self.take_calls()
            .into_iter()
            .filter_map(|call| match call {
                Call::Load(id, _) => Some(id),
                _ => None,
            })
            .collect()
    }
}

fn library_track(id: i64) -> mp_core::Track {
    let mut track = test_track(id, &format!("Track {id}"), "Artist", "Album");
    track.track_number = Some(id as u32);
    track
}

#[test]
fn play_tracks_loads_and_preloads_next() {
    let h = Harness::new(3);
    h.player.play_tracks(Harness::ids(&[1, 2, 3]), 0);
    assert_eq!(
        h.take_calls(),
        vec![Call::Load(1, true), Call::SetNext(Some(2))]
    );
    assert_eq!(h.current(), Some(1));
    assert!(h.player.store().track(TrackId(1)).unwrap().playing());
}

#[test]
fn gapless_transition_advances_queue() {
    let h = Harness::new(3);
    h.player.play_tracks(Harness::ids(&[1, 2, 3]), 0);
    h.started(1, false);
    h.take_calls();
    h.started(2, true);
    assert_eq!(h.player.queue_position(), Some(1));
    assert_eq!(h.current(), Some(2));
    assert_eq!(h.player.duration(), 10_000);
    assert_eq!(h.take_calls(), vec![Call::SetNext(Some(3))]);
    assert!(!h.player.store().track(TrackId(1)).unwrap().playing());
}

#[test]
fn end_of_queue_advances_then_stops() {
    let h = Harness::new(2);
    h.player.play_tracks(Harness::ids(&[1, 2]), 0);
    h.started(1, false);
    h.player.handle_engine_event(EngineEvent::EndOfQueue);
    assert_eq!(h.current(), Some(2));
    h.started(2, false);
    h.player
        .handle_engine_event(EngineEvent::StateChanged(PlaybackState::Playing));
    h.player.handle_engine_event(EngineEvent::EndOfQueue);
    assert_eq!(h.player.state(), PlayerState::Stopped);
    assert_eq!(h.current(), Some(2));
}

#[test]
fn failures_skip_and_stop_when_everything_fails() {
    let h = Harness::new(3);
    h.player.play_tracks(Harness::ids(&[1, 2, 3]), 0);
    for id in 1..=3 {
        h.failed(id);
    }
    assert_eq!(h.player.state(), PlayerState::Stopped);
    assert_eq!(h.errors.borrow().len(), 2, "first failure plus the summary");
    assert_eq!(h.loads(), vec![1, 2, 3]);
}

#[test]
fn failed_preload_is_skipped_while_current_plays() {
    let h = Harness::new(3);
    h.player.play_tracks(Harness::ids(&[1, 2, 3]), 0);
    h.started(1, false);
    h.take_calls();
    h.failed(2);
    assert_eq!(h.current(), Some(1));
    assert_eq!(h.take_calls(), vec![Call::SetNext(Some(3))]);
    assert_eq!(h.errors.borrow().len(), 1);

    h.started(3, true);
    assert_eq!(h.player.queue_position(), Some(2));
    assert_eq!(h.current(), Some(3));
}

#[test]
fn end_of_queue_after_failed_load_does_not_skip_again() {
    let h = Harness::new(3);
    h.player.play_tracks(Harness::ids(&[1, 2, 3]), 0);
    h.failed(1);
    h.player.handle_engine_event(EngineEvent::EndOfQueue);
    assert_eq!(h.current(), Some(2));
    assert_eq!(h.loads(), vec![1, 2]);
}

#[test]
fn current_track_breaking_off_lets_the_engine_move_on() {
    let h = Harness::new(2);
    h.player.play_tracks(Harness::ids(&[1, 2]), 0);
    h.started(1, false);
    h.failed(1);
    assert_eq!(h.player.queue_position(), Some(0), "no second advance");
    h.started(2, true);
    assert_eq!(h.current(), Some(2));

    h.failed(2);
    h.player.handle_engine_event(EngineEvent::EndOfQueue);
    assert_eq!(h.player.state(), PlayerState::Stopped);
    assert!(
        !h.loads().contains(&2),
        "a track that broke off isn't restarted"
    );
}

#[test]
fn events_from_before_a_new_load_are_ignored() {
    let h = Harness::new(3);
    h.player.play_tracks(Harness::ids(&[1, 2, 3]), 0);
    h.started(1, false);
    h.player.jump_to(2);
    // A gapless transition to track 2 that happened before the engine saw the jump.
    h.started(2, true);
    h.player.handle_engine_event(EngineEvent::EndOfQueue);
    assert_eq!(h.player.queue_position(), Some(2));
    assert_eq!(h.current(), Some(3));
    assert_eq!(h.loads(), vec![1, 3]);

    h.started(3, false);
    h.player.handle_engine_event(EngineEvent::EndOfQueue);
    assert_eq!(h.player.state(), PlayerState::Stopped);
}

#[test]
fn started_track_no_longer_queued_is_ignored() {
    let h = Harness::new(2);
    h.player.play_tracks(Harness::ids(&[1, 2]), 0);
    h.player.clear_queue();
    h.started(1, false);
    assert_eq!(h.current(), None);
}

#[test]
fn repeat_one_failure_moves_on() {
    let settings = Settings {
        repeat: RepeatMode::One,
        ..Settings::default()
    };
    let h = Harness::with_settings(2, settings);
    h.player.play_tracks(Harness::ids(&[1, 2]), 0);
    h.failed(1);
    assert_eq!(h.current(), Some(2));
}

#[test]
fn previous_restarts_after_three_seconds() {
    let h = Harness::new(3);
    h.player.play_tracks(Harness::ids(&[1, 2, 3]), 1);
    h.player
        .handle_engine_event(EngineEvent::Seeked(Duration::from_secs(5)));
    h.take_calls();
    h.player.execute(PlayerCommand::Previous);
    assert_eq!(h.take_calls(), vec![Call::Seek(Duration::ZERO)]);
    assert_eq!(h.current(), Some(2));

    h.player.execute(PlayerCommand::Previous);
    assert_eq!(h.current(), Some(1));
}

#[test]
fn property_writes_behave_like_commands() {
    let h = Harness::new(3);
    h.player.play_tracks(Harness::ids(&[1, 2, 3]), 2);
    h.take_calls();

    h.player.set_repeat(PlayerRepeat::All);
    assert_eq!(h.player.repeat_mode(), RepeatMode::All);
    assert_eq!(h.take_calls(), vec![Call::SetNext(Some(1))]);

    h.player.set_shuffle(true);
    assert!(h.player.shuffle());
    assert_eq!(h.current(), Some(3));
    assert!(
        h.events
            .borrow()
            .contains(&PlayerEvent::ShuffleChanged(true))
    );
}

#[test]
fn mute_and_volume_interact() {
    let h = Harness::new(1);
    h.player.set_muted(true);
    assert_eq!(h.take_calls(), vec![Call::Volume(0.0)]);
    h.player.set_volume(0.5);
    assert!(!h.player.muted());
    assert_eq!(h.take_calls(), vec![Call::Volume(0.5)]);
    h.player.execute(PlayerCommand::SetVolume {
        volume: 3.0,
        unmute: true,
    });
    assert_eq!(h.player.volume(), 1.0);
}

/// A plugin fading the volume while the user has muted the player changes the level
/// it comes back at, but the player stays muted, and plugins hear the level.
#[test]
fn plugin_volume_changes_keep_the_mute() {
    let h = Harness::new(1);
    h.player.set_volume(0.8);
    h.player.set_muted(true);
    h.take_calls();
    h.events.borrow_mut().clear();

    h.player.execute(PlayerCommand::SetVolume {
        volume: 0.25,
        unmute: false,
    });
    assert!(h.player.muted());
    assert_eq!(h.player.volume(), 0.25);
    assert_eq!(h.take_calls(), vec![Call::Volume(0.0)]);
    assert_eq!(
        *h.events.borrow(),
        [PlayerEvent::VolumeChanged {
            volume: 0.25,
            muted: true
        }]
    );
}

/// When the output can't be opened while paused, the engine stays paused where it was,
/// so the next Play resumes there rather than starting the song again.
#[test]
fn output_errors_leave_the_state_to_the_engine() {
    let h = Harness::new(1);
    h.player.play_tracks(Harness::ids(&[1]), 0);
    h.started(1, false);
    h.player
        .handle_engine_event(EngineEvent::StateChanged(PlaybackState::Paused));
    h.take_calls();

    h.player
        .handle_engine_event(EngineEvent::OutputError("no device".into()));
    assert_eq!(h.player.state(), PlayerState::Paused);
    assert_eq!(*h.errors.borrow(), ["Audio output failed: no device"]);
    h.player.execute(PlayerCommand::Play);
    assert_eq!(h.take_calls(), vec![Call::Play]);
}

#[test]
fn enqueue_on_empty_queue_loads_paused() {
    let h = Harness::new(2);
    h.player.enqueue(&Harness::ids(&[2]));
    assert_eq!(
        h.take_calls(),
        vec![Call::Load(2, false), Call::SetNext(None)]
    );
    h.player.play_next(&Harness::ids(&[1]));
    assert_eq!(h.player.queue_tracks(), Harness::ids(&[2, 1]));
}

#[test]
fn removing_current_track_moves_on() {
    let h = Harness::new(3);
    h.player.play_tracks(Harness::ids(&[1, 2, 3]), 0);
    h.player.remove_from_queue(0);
    assert_eq!(h.current(), Some(2));
    assert_eq!(h.player.queue_len(), 2);
    h.player.clear_queue();
    assert_eq!(h.current(), None);
    assert_eq!(h.player.queue_len(), 0);
}

#[test]
fn clearing_upcoming_keeps_current_and_history() {
    let h = Harness::new(5);
    h.player.play_tracks(Harness::ids(&[1, 2, 3, 4, 5]), 1);
    let changes = Rc::new(Cell::new(0));
    let counter = changes.clone();
    h.player
        .connect_queue_changed(move |_| counter.set(counter.get() + 1));
    h.take_calls();
    h.player.clear_upcoming();
    assert_eq!(h.player.queue_tracks(), Harness::ids(&[1, 2]));
    assert_eq!(h.current(), Some(2));
    assert_eq!(h.take_calls(), vec![Call::SetNext(None)]);
    assert_eq!(changes.get(), 1);
}

#[test]
fn play_pause_follows_engine_state() {
    let h = Harness::new(2);
    h.player.execute(PlayerCommand::Play);
    assert_eq!(
        h.current(),
        Some(1),
        "play with an empty queue plays the library"
    );
    assert_eq!(h.player.queue_len(), 2);

    h.player
        .handle_engine_event(EngineEvent::StateChanged(PlaybackState::Playing));
    h.take_calls();
    h.player.execute(PlayerCommand::TogglePlayPause);
    h.player
        .handle_engine_event(EngineEvent::StateChanged(PlaybackState::Paused));
    h.player.execute(PlayerCommand::TogglePlayPause);
    h.player.execute(PlayerCommand::Stop);
    assert_eq!(h.take_calls(), vec![Call::Pause, Call::Play, Call::Stop]);
    assert_eq!(h.player.state(), PlayerState::Stopped);
}

#[test]
fn songs_that_leave_the_library_are_no_longer_preloaded() {
    let h = Harness::new(3);
    h.player.play_tracks(Harness::ids(&[1, 2, 3]), 0);
    h.started(1, false);
    h.take_calls();
    h.keep_in_library(&[1, 3]);
    assert_eq!(h.take_calls(), vec![Call::SetNext(Some(3))]);
    assert_eq!(h.current(), Some(1));

    h.started(3, true);
    assert_eq!(h.player.queue_position(), Some(2));
    assert_eq!(h.current(), Some(3));
}

#[test]
fn end_of_queue_passes_over_songs_that_left_the_library() {
    let h = Harness::new(3);
    h.player.play_tracks(Harness::ids(&[1, 2, 3]), 0);
    h.started(1, false);
    h.keep_in_library(&[1, 3]);
    h.take_calls();
    h.player.handle_engine_event(EngineEvent::EndOfQueue);
    assert_eq!(h.loads(), vec![3]);
    assert!(h.errors.borrow().is_empty());
}

#[test]
fn preloaded_song_that_left_the_library_is_not_shown_as_the_previous_one() {
    let h = Harness::new(3);
    h.player.play_tracks(Harness::ids(&[1, 2, 3]), 0);
    h.started(1, false);
    h.keep_in_library(&[1, 3]);
    h.take_calls();
    // The engine had already moved on to track 2 before it saw the new preload.
    h.started(2, true);
    assert_eq!(h.loads(), vec![3]);
    assert_eq!(h.player.queue_position(), Some(2));
    assert_eq!(h.current(), Some(3));
}

#[test]
fn moving_by_hand_retries_songs_that_failed() {
    let h = Harness::new(5);
    h.player.play_tracks(Harness::ids(&[1, 2, 3, 4, 5]), 0);
    for id in 1..=3 {
        h.failed(id);
    }
    assert_eq!(h.current(), Some(4));

    h.player.jump_to(0);
    h.started(1, false);
    assert!(h.take_calls().ends_with(&[Call::SetNext(Some(2))]));

    h.failed(2);
    h.take_calls();
    h.player.execute(PlayerCommand::Next);
    assert_eq!(h.loads(), vec![2]);
}

#[test]
fn new_details_of_the_current_track_are_no_change_of_track() {
    let h = Harness::new(2);
    h.player.play_tracks(Harness::ids(&[1, 2]), 0);
    h.started(1, false);
    h.events.borrow_mut().clear();
    let notified = Rc::new(Cell::new(0));
    let counter = notified.clone();
    h.player
        .connect_current_track_notify(move |_| counter.set(counter.get() + 1));

    let mut retagged = library_track(1);
    retagged.title = "New Title".into();
    h.player
        .store()
        .replace_tracks(vec![retagged, library_track(2)]);
    let current = h.player.current_track().unwrap();
    assert_eq!(current.title(), "New Title");
    assert!(current.playing());
    assert_eq!(notified.get(), 1);
    assert!(
        !h.events
            .borrow()
            .iter()
            .any(|event| matches!(event, PlayerEvent::TrackChanged(_))),
        "{:?}",
        h.events.borrow()
    );
}
