//! The host's view of the player, built from dispatched events, which answers the
//! `mp.player` getters without asking the UI thread.

use std::time::{Duration, Instant};

use mp_core::{PlaybackState, PlayerEvent, RepeatMode, Track};

#[derive(Debug, Clone)]
pub(crate) struct PlayerSnapshot {
    pub state: PlaybackState,
    pub track: Option<Box<Track>>,
    /// The volume level, kept while muted.
    pub volume: f32,
    pub muted: bool,
    pub shuffle: bool,
    pub repeat: RepeatMode,
    /// Last known playback position, as of `position_at`.
    position: Duration,
    position_at: Instant,
}

impl Default for PlayerSnapshot {
    fn default() -> Self {
        Self {
            state: PlaybackState::Stopped,
            track: None,
            volume: 1.0,
            muted: false,
            shuffle: false,
            repeat: RepeatMode::Off,
            position: Duration::ZERO,
            position_at: Instant::now(),
        }
    }
}

impl PlayerSnapshot {
    pub fn apply(&mut self, event: &PlayerEvent) {
        let now = Instant::now();
        match event {
            PlayerEvent::TrackChanged(track) => {
                self.track = track.clone();
                self.set_position(Duration::ZERO, now);
            }
            PlayerEvent::StateChanged(state) => {
                let position = match state {
                    PlaybackState::Stopped => Duration::ZERO,
                    _ => self.position_at(now),
                };
                self.state = *state;
                self.set_position(position, now);
            }
            PlayerEvent::Seeked { position } => self.set_position(*position, now),
            PlayerEvent::VolumeChanged { volume, muted } => {
                self.volume = *volume;
                self.muted = *muted;
            }
            PlayerEvent::ShuffleChanged(shuffle) => self.shuffle = *shuffle,
            PlayerEvent::RepeatChanged(repeat) => self.repeat = *repeat,
            PlayerEvent::LibraryUpdated { .. } => {}
        }
    }

    /// Events that bring a plugin loaded now up to date: volume, shuffle and repeat,
    /// plus the track and the playback state when a track is loaded.
    pub fn state_events(&self) -> Vec<PlayerEvent> {
        let mut events = vec![
            PlayerEvent::VolumeChanged {
                volume: self.volume,
                muted: self.muted,
            },
            PlayerEvent::ShuffleChanged(self.shuffle),
            PlayerEvent::RepeatChanged(self.repeat),
        ];
        if let Some(track) = &self.track {
            events.push(PlayerEvent::TrackChanged(Some(track.clone())));
            events.push(PlayerEvent::StateChanged(self.state));
        }
        events
    }

    /// Estimated position: the last known one plus the time played since, never past
    /// the end of the track.
    pub fn position_at(&self, now: Instant) -> Duration {
        let mut position = self.position;
        if self.state == PlaybackState::Playing {
            position += now.saturating_duration_since(self.position_at);
        }
        match &self.track {
            Some(track) if track.duration_ms > 0 => position.min(track.duration()),
            _ => position,
        }
    }

    fn set_position(&mut self, position: Duration, now: Instant) {
        self.position = position;
        self.position_at = now;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(duration_ms: u64) -> Box<Track> {
        Box::new(Track {
            id: mp_core::TrackId(1),
            path: "/music/a.flac".into(),
            title: "A".into(),
            artist: String::new(),
            album: String::new(),
            album_artist: String::new(),
            genre: String::new(),
            year: None,
            track_number: None,
            disc_number: None,
            duration_ms,
            format: mp_core::AudioFormat::Flac,
            sample_rate: None,
            bit_depth: None,
            channels: None,
            bitrate_kbps: None,
            file_size: 0,
            modified: 0,
            added: 0,
            cover: None,
        })
    }

    #[test]
    fn position_advances_only_while_playing() {
        let mut snapshot = PlayerSnapshot::default();
        snapshot.apply(&PlayerEvent::TrackChanged(Some(track(60_000))));
        snapshot.apply(&PlayerEvent::Seeked {
            position: Duration::from_secs(10),
        });
        let later = Instant::now() + Duration::from_secs(5);
        assert_eq!(snapshot.position_at(later), Duration::from_secs(10));

        snapshot.apply(&PlayerEvent::StateChanged(PlaybackState::Playing));
        let start = snapshot.position_at;
        assert_eq!(
            snapshot.position_at(start + Duration::from_secs(5)),
            Duration::from_secs(15)
        );
        assert_eq!(
            snapshot.position_at(start + Duration::from_secs(500)),
            Duration::from_secs(60),
            "clamped to the track length"
        );

        snapshot.apply(&PlayerEvent::StateChanged(PlaybackState::Stopped));
        assert_eq!(snapshot.position_at(Instant::now()), Duration::ZERO);
    }

    #[test]
    fn pausing_freezes_the_position() {
        let mut snapshot = PlayerSnapshot::default();
        snapshot.apply(&PlayerEvent::TrackChanged(Some(track(0))));
        snapshot.apply(&PlayerEvent::StateChanged(PlaybackState::Playing));
        snapshot.position_at -= Duration::from_secs(3);
        snapshot.apply(&PlayerEvent::StateChanged(PlaybackState::Paused));
        let frozen = snapshot.position_at(Instant::now());
        assert!(frozen >= Duration::from_secs(3));
        let much_later = Instant::now() + Duration::from_secs(60);
        assert_eq!(snapshot.position_at(much_later), frozen);
    }

    #[test]
    fn simple_values_follow_events() {
        let mut snapshot = PlayerSnapshot::default();
        snapshot.apply(&PlayerEvent::VolumeChanged {
            volume: 0.25,
            muted: true,
        });
        snapshot.apply(&PlayerEvent::ShuffleChanged(true));
        snapshot.apply(&PlayerEvent::RepeatChanged(RepeatMode::One));
        snapshot.apply(&PlayerEvent::TrackChanged(None));
        assert_eq!(snapshot.volume, 0.25);
        assert!(snapshot.muted);
        assert!(snapshot.shuffle);
        assert_eq!(snapshot.repeat, RepeatMode::One);
        assert!(snapshot.track.is_none());
    }
}
