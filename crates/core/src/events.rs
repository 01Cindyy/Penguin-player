use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::model::{Track, TrackId};
use crate::queue::RepeatMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlaybackState {
    #[default]
    Stopped,
    Playing,
    Paused,
}

impl PlaybackState {
    pub fn id(self) -> &'static str {
        match self {
            PlaybackState::Stopped => "stopped",
            PlaybackState::Playing => "playing",
            PlaybackState::Paused => "paused",
        }
    }
}

/// Something observable happened in the player. Broadcast by the app to plugins and
/// OS media controls so they never need to reach into UI state.
#[derive(Debug, Clone, PartialEq)]
pub enum PlayerEvent {
    TrackChanged(Option<Box<Track>>),
    StateChanged(PlaybackState),
    Seeked {
        position: Duration,
    },
    /// The volume level or muting changed. `volume` is the level, which muting keeps.
    VolumeChanged {
        volume: f32,
        muted: bool,
    },
    ShuffleChanged(bool),
    RepeatChanged(RepeatMode),
    LibraryUpdated {
        track_count: usize,
    },
}

/// A request to control playback, from the UI, media keys or plugins. The app routes
/// every source through the same handler so behaviour is identical everywhere.
#[derive(Debug, Clone, PartialEq)]
pub enum PlayerCommand {
    Play,
    Pause,
    TogglePlayPause,
    Stop,
    Next,
    Previous,
    SeekTo(Duration),
    /// Relative seek in milliseconds; negative values seek backwards.
    SeekBy(i64),
    /// Sets the linear volume level, `0.0..=1.0`. With `unmute`, a level above zero
    /// also unmutes, as when someone moves a volume control; plugins leave muting to
    /// the user.
    SetVolume {
        volume: f32,
        unmute: bool,
    },
    SetShuffle(bool),
    SetRepeat(RepeatMode),
    /// Replace the queue with `tracks` and start playing at index `start`.
    PlayTracks {
        tracks: Vec<TrackId>,
        start: usize,
    },
    /// Bring the main window to the front.
    Raise,
    Quit,
}
