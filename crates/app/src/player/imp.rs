use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::OnceLock;

use gtk::glib;
use gtk::glib::subclass::Signal;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use mp_core::{PlayQueue, PlayerEvent, TrackId};

use super::{PlaybackBackend, PlayerRepeat, PlayerState};
use crate::model::{LibraryStore, TrackObject};

pub(super) type EventListener = Rc<dyn Fn(&PlayerEvent)>;

#[derive(Default, glib::Properties)]
#[properties(wrapper_type = super::Player)]
pub struct Player {
    #[property(get, builder(PlayerState::default()))]
    pub(super) state: Cell<PlayerState>,
    #[property(get, nullable)]
    pub(super) current_track: RefCell<Option<TrackObject>>,
    /// Position in the current track, in milliseconds. Updated about four times a
    /// second while playing.
    #[property(get)]
    pub(super) position: Cell<u64>,
    /// Length of the current track, in milliseconds.
    #[property(get)]
    pub(super) duration: Cell<u64>,
    /// Linear volume in `0.0..=1.0`, independent of `muted`.
    #[property(get, set = Self::set_volume_property, explicit_notify, minimum = 0.0, maximum = 1.0, default = 1.0)]
    pub(super) volume: Cell<f64>,
    #[property(get, set = Self::set_muted_property, explicit_notify)]
    pub(super) muted: Cell<bool>,
    #[property(get, set = Self::set_shuffle_property, explicit_notify)]
    pub(super) shuffle: Cell<bool>,
    #[property(get, set = Self::set_repeat_property, explicit_notify, builder(PlayerRepeat::default()))]
    pub(super) repeat: Cell<PlayerRepeat>,

    pub(super) backend: RefCell<Option<Box<dyn PlaybackBackend>>>,
    pub(super) queue: RefCell<PlayQueue>,
    pub(super) store: OnceCell<LibraryStore>,
    pub(super) listeners: RefCell<Vec<EventListener>>,
    pub(super) position_timer: RefCell<Option<glib::SourceId>>,
    /// The last `load` sent to the engine, until the engine reports how it went.
    /// Engine events that arrive before that report describe an older state.
    pub(super) pending_load: Cell<Option<PendingLoad>>,
    /// Tracks the engine couldn't play since the queue was filled or the user last
    /// moved through it. Automatic moves (gapless preloading, skipping after a
    /// failure) pass over them.
    pub(super) unplayable: RefCell<HashSet<TrackId>>,
    /// Tracks that failed in a row without another one starting in between.
    pub(super) failure_streak: Cell<usize>,
    pub(super) startup_error: RefCell<Option<String>>,
    /// The audio being played, for the visualiser; `None` without an engine.
    pub(super) scope: OnceCell<mp_audio::Scope>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PendingLoad {
    pub(super) track: TrackId,
    pub(super) autoplay: bool,
}

// Property writes (e.g. from a two-way binding to a slider) go through the same code
// paths as `Player::execute`, so every source of a change behaves identically.
impl Player {
    fn set_volume_property(&self, volume: f64) {
        self.obj().apply_volume(volume, true);
    }

    fn set_muted_property(&self, muted: bool) {
        self.obj().apply_muted(muted);
    }

    fn set_shuffle_property(&self, shuffle: bool) {
        self.obj().apply_shuffle(shuffle);
    }

    fn set_repeat_property(&self, repeat: PlayerRepeat) {
        self.obj().apply_repeat(repeat.into());
    }
}

#[glib::object_subclass]
impl ObjectSubclass for Player {
    const NAME: &'static str = "MpPlayer";
    type Type = super::Player;
}

#[glib::derived_properties]
impl ObjectImpl for Player {
    fn signals() -> &'static [Signal] {
        static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
        SIGNALS.get_or_init(|| {
            vec![
                Signal::builder("queue-changed").build(),
                Signal::builder("playback-error")
                    .param_types([String::static_type()])
                    .build(),
                Signal::builder("raise-requested").build(),
                Signal::builder("quit-requested").build(),
            ]
        })
    }

    fn dispose(&self) {
        if let Some(timer) = self.position_timer.take() {
            timer.remove();
        }
        self.listeners.borrow_mut().clear();
    }
}
