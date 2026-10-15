//! OS media controls: MPRIS over D-Bus on Linux and the BSDs, Now Playing on macOS,
//! nothing elsewhere yet. Everything platform specific in the app lives here.
//!
//! Media keys and desktop widgets become player commands, and the player's events
//! update what the desktop shows. A missing D-Bus session only disables the feature.

use std::rc::Rc;

use crate::context::AppContext;

/// Connects the player to the platform's media controls, if it has any.
pub fn start(ctx: &Rc<AppContext>) {
    platform::start(ctx);
}

#[cfg(all(unix, not(any(target_os = "android", target_os = "ios"))))]
mod platform {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::Duration;

    use gtk::{gio, glib};
    use mp_core::{APP_ID, APP_NAME, PlaybackState, PlayerCommand, PlayerEvent, Track};
    use souvlaki::{
        MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition,
        PlatformConfig, SeekDirection,
    };

    use crate::context::AppContext;

    /// How far the "seek forward/backward" media keys jump.
    const SEEK_STEP_MS: i64 = 5_000;

    /// `None` until connected, and again if the controls stop working.
    type SharedControls = Rc<RefCell<Option<MediaControls>>>;

    pub fn start(ctx: &Rc<AppContext>) {
        let controls: SharedControls = Rc::default();
        let (events, incoming) = async_channel::unbounded::<MediaControlEvent>();

        let weak = Rc::downgrade(ctx);
        let shared = controls.clone();
        connect(events, move |result| match result {
            Ok(attached) => {
                shared.replace(Some(attached));
                if let Some(ctx) = weak.upgrade() {
                    publish_current_state(&ctx, &shared);
                }
            }
            Err(err) => log::info!("Media controls unavailable: {err}"),
        });

        // The current track's details also change when a rescan updates its tags or
        // cover, which is no change of track for the events below.
        let weak = Rc::downgrade(ctx);
        let shared = controls.clone();
        ctx.player.connect_current_track_notify(move |_| {
            if let Some(ctx) = weak.upgrade() {
                update(&shared, |media| publish_metadata(&ctx, media));
            }
        });

        let weak = Rc::downgrade(ctx);
        ctx.player.connect_player_event(move |event| {
            if let Some(ctx) = weak.upgrade() {
                update(&controls, |media| publish(&ctx, media, event));
            }
        });

        let weak = Rc::downgrade(ctx);
        glib::spawn_future_local(async move {
            while let Ok(event) = incoming.recv().await {
                let Some(ctx) = weak.upgrade() else { break };
                if let Some(command) = translate(event) {
                    ctx.player.execute(command);
                }
            }
        });
    }

    fn create(events: async_channel::Sender<MediaControlEvent>) -> Result<MediaControls, String> {
        let config = PlatformConfig {
            display_name: APP_NAME,
            dbus_name: APP_ID,
            hwnd: None,
        };
        let mut controls = MediaControls::new(config).map_err(|err| err.to_string())?;
        controls
            .attach(move |event| {
                let _ = events.send_blocking(event);
            })
            .map_err(|err| err.to_string())?;
        Ok(controls)
    }

    /// Connecting to the session bus can stall, so on D-Bus platforms it happens on a
    /// short-lived thread and the attached controls are handed back to the main thread.
    #[cfg(not(target_os = "macos"))]
    fn connect(
        events: async_channel::Sender<MediaControlEvent>,
        done: impl FnOnce(Result<MediaControls, String>) + 'static,
    ) {
        let (sender, receiver) = async_channel::bounded(1);
        let spawned = std::thread::Builder::new()
            .name("media-controls".into())
            .spawn(move || {
                let _ = sender.send_blocking(create(events));
            });
        if let Err(err) = spawned {
            done(Err(err.to_string()));
            return;
        }
        glib::spawn_future_local(async move {
            if let Ok(result) = receiver.recv().await {
                done(result);
            }
        });
    }

    /// macOS requires Now Playing to be set up on the main thread; it doesn't block.
    #[cfg(target_os = "macos")]
    fn connect(
        events: async_channel::Sender<MediaControlEvent>,
        done: impl FnOnce(Result<MediaControls, String>) + 'static,
    ) {
        done(create(events));
    }

    pub(super) fn translate(event: MediaControlEvent) -> Option<PlayerCommand> {
        let seek_by = |direction: SeekDirection, millis: i64| {
            PlayerCommand::SeekBy(match direction {
                SeekDirection::Forward => millis,
                SeekDirection::Backward => -millis,
            })
        };
        let command = match event {
            MediaControlEvent::Play => PlayerCommand::Play,
            MediaControlEvent::Pause => PlayerCommand::Pause,
            MediaControlEvent::Toggle => PlayerCommand::TogglePlayPause,
            MediaControlEvent::Next => PlayerCommand::Next,
            MediaControlEvent::Previous => PlayerCommand::Previous,
            MediaControlEvent::Stop => PlayerCommand::Stop,
            MediaControlEvent::Seek(direction) => seek_by(direction, SEEK_STEP_MS),
            MediaControlEvent::SeekBy(direction, by) => {
                seek_by(direction, by.as_millis().try_into().unwrap_or(i64::MAX))
            }
            MediaControlEvent::SetPosition(MediaPosition(position)) => {
                PlayerCommand::SeekTo(position)
            }
            MediaControlEvent::SetVolume(volume) => PlayerCommand::SetVolume {
                volume: volume.clamp(0.0, 1.0) as f32,
                unmute: true,
            },
            MediaControlEvent::Raise => PlayerCommand::Raise,
            MediaControlEvent::Quit => PlayerCommand::Quit,
            MediaControlEvent::OpenUri(uri) => {
                log::debug!("Ignoring media controls request to open {uri}");
                return None;
            }
        };
        Some(command)
    }

    /// Sends everything at once, for when the controls have just connected.
    fn publish_current_state(ctx: &AppContext, controls: &SharedControls) {
        update(controls, |media| publish_metadata(ctx, media));
        for event in ctx.player.state_events() {
            update(controls, |media| publish(ctx, media, &event));
        }
    }

    /// Runs `change` on the controls if they are connected, and drops them if they fail.
    fn update(
        controls: &SharedControls,
        change: impl FnOnce(&mut MediaControls) -> Result<(), souvlaki::Error>,
    ) {
        let mut slot = controls.borrow_mut();
        let Some(media) = slot.as_mut() else { return };
        if let Err(err) = change(media) {
            log::warn!("Media controls stopped working: {err}");
            slot.take();
        }
    }

    fn publish_metadata(
        ctx: &AppContext,
        media: &mut MediaControls,
    ) -> Result<(), souvlaki::Error> {
        let Some(object) = ctx.player.current_track() else {
            return media.set_metadata(MediaMetadata::default());
        };
        let track = object.track();
        let cover_url = cover_url(ctx, track);
        media.set_metadata(MediaMetadata {
            title: Some(&track.title),
            album: Some(track.display_album()),
            artist: Some(track.display_artist()),
            cover_url: cover_url.as_deref(),
            duration: Some(track.duration()),
        })
    }

    fn publish(
        ctx: &AppContext,
        media: &mut MediaControls,
        event: &PlayerEvent,
    ) -> Result<(), souvlaki::Error> {
        match event {
            // Desktops extrapolate the position from the last one published, and a
            // gapless change of track publishes no other state.
            PlayerEvent::TrackChanged(Some(_)) => {
                media.set_playback(playback(ctx.player.playback_state(), 0))
            }
            PlayerEvent::StateChanged(state) => {
                media.set_playback(playback(*state, ctx.player.position()))
            }
            PlayerEvent::Seeked { position } => media.set_playback(playback(
                ctx.player.playback_state(),
                position.as_millis() as u64,
            )),
            // Desktops have no mute for players, so muted shows as silent.
            PlayerEvent::VolumeChanged { volume, muted } => {
                set_volume(media, if *muted { 0.0 } else { f64::from(*volume) })
            }
            _ => Ok(()),
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn set_volume(media: &mut MediaControls, volume: f64) -> Result<(), souvlaki::Error> {
        media.set_volume(volume)
    }

    /// Now Playing has no volume.
    #[cfg(target_os = "macos")]
    fn set_volume(_media: &mut MediaControls, _volume: f64) -> Result<(), souvlaki::Error> {
        Ok(())
    }

    fn playback(state: PlaybackState, position_ms: u64) -> MediaPlayback {
        let progress = Some(MediaPosition(Duration::from_millis(position_ms)));
        match state {
            PlaybackState::Playing => MediaPlayback::Playing { progress },
            PlaybackState::Paused => MediaPlayback::Paused { progress },
            PlaybackState::Stopped => MediaPlayback::Stopped,
        }
    }

    /// The full-size cover as a `file://` URL, which is what desktop widgets expect.
    fn cover_url(ctx: &AppContext, track: &Track) -> Option<String> {
        let path = ctx.covers.source_path(track.cover.as_ref()?);
        path.exists()
            .then(|| gio::prelude::FileExt::uri(&gio::File::for_path(&path)).to_string())
    }
}

#[cfg(not(all(unix, not(any(target_os = "android", target_os = "ios")))))]
mod platform {
    use std::rc::Rc;

    use crate::context::AppContext;

    /// Windows media controls need a native window handle; not wired up yet.
    pub fn start(_ctx: &Rc<AppContext>) {}
}

#[cfg(all(test, unix, not(any(target_os = "android", target_os = "ios"))))]
mod tests {
    use std::time::Duration;

    use mp_core::PlayerCommand;
    use souvlaki::{MediaControlEvent, MediaPosition, SeekDirection};

    use super::platform::translate;

    #[test]
    fn media_events_become_commands() {
        assert_eq!(
            translate(MediaControlEvent::Toggle),
            Some(PlayerCommand::TogglePlayPause)
        );
        assert_eq!(
            translate(MediaControlEvent::Seek(SeekDirection::Backward)),
            Some(PlayerCommand::SeekBy(-5_000))
        );
        assert_eq!(
            translate(MediaControlEvent::SetPosition(MediaPosition(
                Duration::from_secs(42)
            ))),
            Some(PlayerCommand::SeekTo(Duration::from_secs(42)))
        );
        assert_eq!(
            translate(MediaControlEvent::SetVolume(3.0)),
            Some(PlayerCommand::SetVolume {
                volume: 1.0,
                unmute: true
            })
        );
        assert_eq!(translate(MediaControlEvent::OpenUri("x".into())), None);
    }
}
