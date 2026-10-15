//! The example plugins in `examples/plugins/`, run through the real host.

mod common;

use std::path::PathBuf;
use std::time::Duration;

use common::{TestDirs, start, track, write};
use mp_core::{AudioFormat, PlaybackState, PlayerCommand, PlayerEvent};
use mp_plugins::discover;

fn examples_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/plugins")
}

#[test]
fn examples_are_valid_plugins() {
    let plugins: Vec<_> = discover(&examples_dir())
        .into_iter()
        .filter(|plugin| !plugin.is_bundled())
        .collect();
    let ids: Vec<&str> = plugins.iter().map(|plugin| plugin.id.as_str()).collect();
    assert_eq!(ids, ["now-playing-toast", "sleep-timer"]);
    for plugin in &plugins {
        assert_eq!(plugin.problem, None, "{}", plugin.id);
        assert!(!plugin.manifest.description.is_empty());
    }
}

#[test]
fn now_playing_toast_announces_tracks() {
    let dirs = TestDirs::new();
    let (host, events) = start(examples_dir(), dirs.data(), &["now-playing-toast"]);

    host.dispatch(PlayerEvent::TrackChanged(Some(Box::new(track()))));
    assert_eq!(
        events.notification(),
        (
            "Song".to_owned(),
            Some("Artist — Album · 24-bit / 96 kHz FLAC".to_owned())
        )
    );

    let mut alac = track();
    alac.title = "Shinjuku 2 a.m.".into();
    alac.artist = "Kōji Arai".into();
    alac.album = "Night Transit".into();
    alac.path = "/music/Kōji Arai/Night Transit/1-01 Shinjuku 2 a.m..m4a".into();
    alac.format = AudioFormat::Alac;
    alac.sample_rate = Some(48_000);
    host.dispatch(PlayerEvent::TrackChanged(Some(Box::new(alac))));
    assert_eq!(
        events.notification(),
        (
            "Shinjuku 2 a.m.".to_owned(),
            Some("Kōji Arai — Night Transit · 24-bit / 48 kHz ALAC".to_owned())
        )
    );

    let mut mp3 = track();
    mp3.title = "Untagged".into();
    mp3.artist.clear();
    mp3.album.clear();
    mp3.format = AudioFormat::Mp3;
    mp3.bit_depth = None;
    mp3.sample_rate = Some(44_100);
    mp3.bitrate_kbps = Some(320);
    host.dispatch(PlayerEvent::TrackChanged(Some(Box::new(mp3))));
    assert_eq!(
        events.notification(),
        ("Untagged".to_owned(), Some("320 kbps MP3".to_owned()))
    );

    host.dispatch(PlayerEvent::TrackChanged(None));
    events.expect_none(Duration::from_millis(100));
}

#[test]
fn sleep_timer_saves_its_default_settings() {
    let dirs = TestDirs::new();
    let (host, events) = start(examples_dir(), dirs.data(), &["sleep-timer"]);
    drop(host);
    events.expect_none(Duration::ZERO);
    let saved = std::fs::read_to_string(dirs.data().join("sleep-timer.json")).unwrap();
    let saved: serde_json::Value = serde_json::from_str(&saved).unwrap();
    assert_eq!(
        saved,
        serde_json::json!({ "minutes": 30, "fade_seconds": 10 })
    );
}

#[test]
fn sleep_timer_fades_out_and_pauses() {
    let dirs = TestDirs::new();
    // 60 ms of playback, then a ten step fade lasting 100 ms.
    write(
        &dirs.data().join("sleep-timer.json"),
        r#"{ "minutes": 0.001, "fade_seconds": 0.1 }"#,
    );
    let (host, events) = start(examples_dir(), dirs.data(), &["sleep-timer"]);
    host.dispatch(PlayerEvent::VolumeChanged {
        volume: 0.8,
        muted: false,
    });
    host.dispatch(PlayerEvent::StateChanged(PlaybackState::Playing));

    for step in 1..10 {
        let expected = 0.8 * (1.0 - step as f32 / 10.0);
        match events.command() {
            PlayerCommand::SetVolume { volume, .. } => {
                assert!((volume - expected).abs() < 1e-4, "step {step}: {volume}");
            }
            other => panic!("expected a volume change, got {other:?}"),
        }
    }
    assert_eq!(events.command(), PlayerCommand::Pause);
    assert_eq!(
        events.command(),
        PlayerCommand::SetVolume {
            volume: 0.8,
            unmute: false,
        }
    );
    let (title, body) = events.notification();
    assert_eq!(title, "Sleep timer");
    assert!(body.unwrap().contains("Paused after 0.001 minutes"));

    // The app reports the pause; nothing else should happen.
    host.dispatch(PlayerEvent::StateChanged(PlaybackState::Paused));
    events.expect_none(Duration::from_millis(200));
}

#[test]
fn sleep_timer_is_cancelled_by_pausing() {
    let dirs = TestDirs::new();
    write(
        &dirs.data().join("sleep-timer.json"),
        r#"{ "minutes": 0.003, "fade_seconds": 0 }"#,
    );
    let (host, events) = start(examples_dir(), dirs.data(), &["sleep-timer"]);
    host.dispatch(PlayerEvent::StateChanged(PlaybackState::Playing));
    host.dispatch(PlayerEvent::StateChanged(PlaybackState::Paused));
    events.expect_none(Duration::from_millis(400));

    // Playing again starts a fresh countdown, which pauses straight away without a fade.
    host.dispatch(PlayerEvent::StateChanged(PlaybackState::Playing));
    assert_eq!(events.command(), PlayerCommand::Pause);
    events.notified("Sleep timer");
}
