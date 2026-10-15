use gtk::glib;
use mp_core::{PlaybackState, RepeatMode};

/// [`PlaybackState`] as a GObject enum, so it can be a property.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, glib::Enum)]
#[enum_type(name = "MpPlayerState")]
pub enum PlayerState {
    #[default]
    Stopped,
    Playing,
    Paused,
}

impl From<PlaybackState> for PlayerState {
    fn from(state: PlaybackState) -> Self {
        match state {
            PlaybackState::Stopped => PlayerState::Stopped,
            PlaybackState::Playing => PlayerState::Playing,
            PlaybackState::Paused => PlayerState::Paused,
        }
    }
}

impl From<PlayerState> for PlaybackState {
    fn from(state: PlayerState) -> Self {
        match state {
            PlayerState::Stopped => PlaybackState::Stopped,
            PlayerState::Playing => PlaybackState::Playing,
            PlayerState::Paused => PlaybackState::Paused,
        }
    }
}

/// [`RepeatMode`] as a GObject enum, so it can be a property.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, glib::Enum)]
#[enum_type(name = "MpPlayerRepeat")]
pub enum PlayerRepeat {
    #[default]
    Off,
    All,
    One,
}

impl From<RepeatMode> for PlayerRepeat {
    fn from(mode: RepeatMode) -> Self {
        match mode {
            RepeatMode::Off => PlayerRepeat::Off,
            RepeatMode::All => PlayerRepeat::All,
            RepeatMode::One => PlayerRepeat::One,
        }
    }
}

impl From<PlayerRepeat> for RepeatMode {
    fn from(mode: PlayerRepeat) -> Self {
        match mode {
            PlayerRepeat::Off => RepeatMode::Off,
            PlayerRepeat::All => RepeatMode::All,
            PlayerRepeat::One => RepeatMode::One,
        }
    }
}
