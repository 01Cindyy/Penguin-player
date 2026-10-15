//! End-to-end tests of the playback engine with the null output, using short WAV files
//! generated in a temporary directory.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use async_channel::Receiver;
use mp_audio::{AudioEngine, EngineConfig, EngineEvent};
use mp_core::{PlaybackState, TrackId};
use tempfile::TempDir;

const RATE: u32 = 8_000;
/// Generous bound for anything the engine should do "right away".
const TIMEOUT: Duration = Duration::from_secs(5);

struct Fixture {
    engine: AudioEngine,
    events: Receiver<EngineEvent>,
    dir: TempDir,
}

impl Fixture {
    fn new() -> Self {
        let config = EngineConfig {
            null_output: true,
            ..EngineConfig::default()
        };
        let (engine, events) = AudioEngine::new(config).expect("engine starts");
        let dir = tempfile::tempdir().expect("temp dir");
        Self {
            engine,
            events,
            dir,
        }
    }

    /// Writes a 16-bit WAV file with a quiet tone and returns its path.
    fn wav(&self, name: &str, rate: u32, channels: u16, seconds: f32) -> PathBuf {
        let path = self.dir.path().join(name);
        let frames = (rate as f32 * seconds) as u32;
        common::write_wav(&path, rate, channels, frames, common::tone(rate));
        path
    }

    fn garbage(&self, name: &str) -> PathBuf {
        let path = self.dir.path().join(name);
        std::fs::write(&path, b"this is not audio at all, just some text").expect("write");
        path
    }

    /// Waits for the next event.
    fn next_event(&self) -> EngineEvent {
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            if let Ok(event) = self.events.try_recv() {
                return event;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("no event within {TIMEOUT:?}");
    }

    /// Asserts that no event arrives for `duration`.
    fn assert_no_event(&self, duration: Duration) {
        std::thread::sleep(duration);
        if let Ok(event) = self.events.try_recv() {
            panic!("unexpected event {event:?}");
        }
    }

    fn expect_started(&self, track: i64, from_queue: bool) {
        match self.next_event() {
            EngineEvent::TrackStarted {
                track: started,
                from_queue: queued,
                ..
            } => assert_eq!((started, queued), (TrackId(track), from_queue)),
            other => panic!("expected TrackStarted for {track}, got {other:?}"),
        }
    }

    fn expect(&self, expected: EngineEvent) {
        assert_eq!(self.next_event(), expected);
    }

    fn play_from_start(&self, track: i64, path: PathBuf) {
        self.engine.load(TrackId(track), path, true);
        self.expect_started(track, false);
        self.expect(EngineEvent::StateChanged(PlaybackState::Playing));
    }
}

fn assert_near(actual: Duration, expected: Duration, tolerance: Duration) {
    let difference = actual.abs_diff(expected);
    assert!(
        difference <= tolerance,
        "{actual:?} is not within {tolerance:?} of {expected:?}"
    );
}

#[test]
fn load_and_play_reports_track_then_state() {
    let fixture = Fixture::new();
    let path = fixture.wav("a.wav", RATE, 2, 1.0);
    fixture.engine.load(TrackId(1), path, true);

    match fixture.next_event() {
        EngineEvent::TrackStarted {
            track,
            duration,
            from_queue,
        } => {
            assert_eq!(track, TrackId(1));
            assert!(!from_queue);
            assert_near(
                duration.expect("WAV has a duration"),
                Duration::from_secs(1),
                Duration::from_millis(1),
            );
        }
        other => panic!("expected TrackStarted, got {other:?}"),
    }
    fixture.expect(EngineEvent::StateChanged(PlaybackState::Playing));
    assert_eq!(fixture.engine.state(), PlaybackState::Playing);
}

#[test]
fn position_follows_real_time() {
    let fixture = Fixture::new();
    fixture.play_from_start(1, fixture.wav("a.wav", RATE, 2, 2.0));
    let started = Instant::now();
    std::thread::sleep(Duration::from_millis(600));
    assert_near(
        fixture.engine.position(),
        started.elapsed(),
        Duration::from_millis(150),
    );
}

#[test]
fn pause_holds_position_and_play_resumes() {
    let fixture = Fixture::new();
    fixture.play_from_start(1, fixture.wav("a.wav", RATE, 2, 2.0));
    std::thread::sleep(Duration::from_millis(300));

    fixture.engine.pause();
    fixture.expect(EngineEvent::StateChanged(PlaybackState::Paused));
    std::thread::sleep(Duration::from_millis(50));
    let paused_at = fixture.engine.position();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(fixture.engine.position(), paused_at);
    assert_eq!(fixture.engine.state(), PlaybackState::Paused);

    fixture.engine.play();
    fixture.expect(EngineEvent::StateChanged(PlaybackState::Playing));
    std::thread::sleep(Duration::from_millis(300));
    assert_near(
        fixture.engine.position(),
        paused_at + Duration::from_millis(300),
        Duration::from_millis(150),
    );
}

#[test]
fn seek_while_playing_jumps_at_once() {
    let fixture = Fixture::new();
    fixture.play_from_start(1, fixture.wav("a.wav", RATE, 2, 3.0));
    std::thread::sleep(Duration::from_millis(100));

    fixture.engine.seek(Duration::from_secs(2));
    match fixture.next_event() {
        EngineEvent::Seeked(position) => {
            assert_near(position, Duration::from_secs(2), Duration::from_millis(1));
        }
        other => panic!("expected Seeked, got {other:?}"),
    }
    assert_near(
        fixture.engine.position(),
        Duration::from_secs(2),
        Duration::from_millis(50),
    );
    std::thread::sleep(Duration::from_millis(300));
    assert_near(
        fixture.engine.position(),
        Duration::from_millis(2300),
        Duration::from_millis(150),
    );
}

#[test]
fn seek_while_paused_stays_paused() {
    let fixture = Fixture::new();
    fixture
        .engine
        .load(TrackId(1), fixture.wav("a.wav", RATE, 2, 3.0), false);
    fixture.expect_started(1, false);
    fixture.expect(EngineEvent::StateChanged(PlaybackState::Paused));
    assert_eq!(fixture.engine.position(), Duration::ZERO);

    fixture.engine.seek(Duration::from_millis(1500));
    fixture.expect(EngineEvent::Seeked(Duration::from_millis(1500)));
    fixture.assert_no_event(Duration::from_millis(200));
    assert_eq!(fixture.engine.position(), Duration::from_millis(1500));
    assert_eq!(fixture.engine.state(), PlaybackState::Paused);

    fixture.engine.play();
    fixture.expect(EngineEvent::StateChanged(PlaybackState::Playing));
    std::thread::sleep(Duration::from_millis(300));
    assert_near(
        fixture.engine.position(),
        Duration::from_millis(1800),
        Duration::from_millis(150),
    );
}

#[test]
fn seeking_to_the_end_moves_on_to_the_next_track() {
    let fixture = Fixture::new();
    fixture.play_from_start(1, fixture.wav("a.wav", RATE, 2, 3.0));
    fixture
        .engine
        .set_next(Some((TrackId(2), fixture.wav("b.wav", RATE, 2, 0.3))));

    fixture.engine.seek(Duration::from_secs(60));
    fixture.expect(EngineEvent::Seeked(Duration::from_secs(3)));
    fixture.expect_started(2, true);
    fixture.expect(EngineEvent::EndOfQueue);
}

#[test]
fn seeking_back_in_a_fully_buffered_m4a_keeps_playing() {
    let fixture = Fixture::new();
    // Shorter than the decode-ahead window, so the whole file has been read when it seeks.
    let path = fixture.dir.path().join("a.m4a");
    common::write_m4a(&path, RATE, RATE * 3 / 10, common::tone(RATE));
    fixture.play_from_start(1, path);
    std::thread::sleep(Duration::from_millis(100));

    fixture.engine.seek(Duration::from_millis(50));
    fixture.expect(EngineEvent::Seeked(Duration::from_millis(50)));
    fixture.expect(EngineEvent::EndOfQueue);
}

#[test]
fn gapless_next_track_then_end_of_queue() {
    let fixture = Fixture::new();
    let first = fixture.wav("a.wav", RATE, 2, 0.5);
    let second = fixture.wav("b.wav", RATE, 2, 0.5);
    fixture.play_from_start(1, first);
    let started = Instant::now();
    fixture.engine.set_next(Some((TrackId(2), second)));

    fixture.expect_started(2, true);
    // The boundary is reported when it is heard, not when it is decoded.
    assert_near(
        started.elapsed(),
        Duration::from_millis(500),
        Duration::from_millis(150),
    );
    assert!(fixture.engine.position() < Duration::from_millis(100));

    fixture.expect(EngineEvent::EndOfQueue);
    fixture.expect(EngineEvent::StateChanged(PlaybackState::Stopped));
    assert_near(
        started.elapsed(),
        Duration::from_millis(1000),
        Duration::from_millis(200),
    );
    assert_eq!(fixture.engine.position(), Duration::ZERO);
}

#[test]
fn replacing_the_queued_track_skips_it() {
    let fixture = Fixture::new();
    // Shorter than the decode-ahead window, so the queued track's audio is already in the
    // output buffer when it is replaced.
    fixture.play_from_start(1, fixture.wav("a.wav", RATE, 2, 0.3));
    fixture
        .engine
        .set_next(Some((TrackId(2), fixture.wav("b.wav", RATE, 2, 0.3))));
    std::thread::sleep(Duration::from_millis(50));
    fixture
        .engine
        .set_next(Some((TrackId(3), fixture.wav("c.wav", RATE, 2, 0.3))));

    fixture.expect_started(3, true);
    fixture.expect(EngineEvent::EndOfQueue);
}

#[test]
fn changing_the_next_track_rapidly_up_to_the_boundary_starts_one_track() {
    let fixture = Fixture::new();
    // The next track is replaced hundreds of times while its audio is already in the
    // output, more often than the output has room to cut it out seamlessly.
    fixture.play_from_start(1, fixture.wav("a.wav", RATE, 2, 0.5));
    let candidates = [
        (TrackId(2), fixture.wav("b.wav", RATE, 2, 0.3)),
        (TrackId(3), fixture.wav("c.wav", RATE, 2, 0.3)),
    ];
    // Like the app, keep changing it until the engine reports moving on.
    let deadline = Instant::now() + TIMEOUT;
    let started = loop {
        assert!(Instant::now() < deadline, "the next track never started");
        for next in &candidates {
            fixture.engine.set_next(Some(next.clone()));
            std::thread::sleep(Duration::from_millis(1));
        }
        if let Ok(event) = fixture.events.try_recv() {
            break event;
        }
    };
    fixture.engine.set_next(None);

    assert!(
        matches!(started, EngineEvent::TrackStarted { from_queue: true, track, .. }
            if track == TrackId(2) || track == TrackId(3)),
        "expected the next track to start, got {started:?}"
    );
    fixture.expect(EngineEvent::EndOfQueue);
    fixture.expect(EngineEvent::StateChanged(PlaybackState::Stopped));
    fixture.assert_no_event(Duration::from_millis(100));
}

#[test]
fn queuing_the_playing_track_again_repeats_it() {
    let fixture = Fixture::new();
    let path = fixture.wav("a.wav", RATE, 2, 0.3);
    fixture.play_from_start(1, path.clone());
    // What the app does for "repeat one".
    fixture.engine.set_next(Some((TrackId(1), path)));

    fixture.expect_started(1, true);
    fixture.expect(EngineEvent::EndOfQueue);
}

#[test]
fn next_track_changed_while_paused_follows_on_play() {
    let fixture = Fixture::new();
    // Short enough that both tracks are decoded before the pause.
    fixture.play_from_start(1, fixture.wav("a.wav", RATE, 2, 0.3));
    let second = fixture.wav("b.wav", RATE, 2, 0.3);
    let third = fixture.wav("c.wav", RATE, 2, 0.3);
    fixture.engine.set_next(Some((TrackId(2), second.clone())));
    std::thread::sleep(Duration::from_millis(50));
    fixture.engine.pause();
    fixture.expect(EngineEvent::StateChanged(PlaybackState::Paused));

    for _ in 0..50 {
        fixture.engine.set_next(Some((TrackId(2), second.clone())));
        fixture.engine.set_next(Some((TrackId(3), third.clone())));
    }
    fixture.assert_no_event(Duration::from_millis(100));
    fixture.engine.play();
    fixture.expect(EngineEvent::StateChanged(PlaybackState::Playing));
    fixture.expect_started(3, true);
    fixture.expect(EngineEvent::EndOfQueue);
}

#[test]
fn next_track_with_another_format_follows_after_a_reopen() {
    let fixture = Fixture::new();
    fixture.play_from_start(1, fixture.wav("a.wav", RATE, 2, 0.4));
    let mono_hi_rate = fixture.wav("b.wav", 2 * RATE, 1, 0.4);
    fixture.engine.set_next(Some((TrackId(2), mono_hi_rate)));

    fixture.expect_started(2, true);
    fixture.expect(EngineEvent::EndOfQueue);
}

#[test]
fn without_gapless_the_next_track_still_follows() {
    let fixture = Fixture::new();
    fixture.engine.set_gapless(false);
    fixture.play_from_start(1, fixture.wav("a.wav", RATE, 2, 0.4));
    fixture
        .engine
        .set_next(Some((TrackId(2), fixture.wav("b.wav", RATE, 2, 0.4))));

    fixture.expect_started(2, true);
    fixture.expect(EngineEvent::EndOfQueue);
}

#[test]
fn load_while_playing_replaces_the_track() {
    let fixture = Fixture::new();
    fixture.play_from_start(1, fixture.wav("a.wav", RATE, 2, 2.0));
    std::thread::sleep(Duration::from_millis(300));

    fixture
        .engine
        .load(TrackId(2), fixture.wav("b.wav", RATE, 2, 2.0), true);
    // Already playing, so there is no state change.
    fixture.expect_started(2, false);
    assert!(fixture.engine.position() < Duration::from_millis(50));
    std::thread::sleep(Duration::from_millis(200));
    assert_near(
        fixture.engine.position(),
        Duration::from_millis(200),
        Duration::from_millis(120),
    );
    fixture.assert_no_event(Duration::from_millis(100));
}

#[test]
fn unreadable_file_fails_without_playing() {
    let fixture = Fixture::new();
    fixture
        .engine
        .load(TrackId(9), fixture.garbage("notes.wav"), true);
    match fixture.next_event() {
        EngineEvent::TrackFailed { track, message } => {
            assert_eq!(track, TrackId(9));
            assert!(!message.is_empty());
        }
        other => panic!("expected TrackFailed, got {other:?}"),
    }
    fixture.assert_no_event(Duration::from_millis(100));
    assert_eq!(fixture.engine.state(), PlaybackState::Stopped);
}

#[test]
fn corrupt_sample_rate_fails_without_playing() {
    let fixture = Fixture::new();
    let path = fixture.dir.path().join("corrupt.wav");
    common::write_wav(&path, 2_000_000_000, 1, 200, |frame| frame as i16);
    fixture.engine.load(TrackId(4), path, true);
    match fixture.next_event() {
        EngineEvent::TrackFailed { track, .. } => assert_eq!(track, TrackId(4)),
        other => panic!("expected TrackFailed, got {other:?}"),
    }
    assert_eq!(fixture.engine.state(), PlaybackState::Stopped);
}

#[test]
fn broken_next_track_is_reported_and_can_be_replaced() {
    let fixture = Fixture::new();
    fixture.play_from_start(1, fixture.wav("a.wav", RATE, 2, 0.6));
    fixture
        .engine
        .set_next(Some((TrackId(2), fixture.garbage("broken.flac"))));
    match fixture.next_event() {
        EngineEvent::TrackFailed { track, .. } => assert_eq!(track, TrackId(2)),
        other => panic!("expected TrackFailed, got {other:?}"),
    }
    // The current track keeps playing; the app queues the track after the broken one.
    assert_eq!(fixture.engine.state(), PlaybackState::Playing);
    fixture
        .engine
        .set_next(Some((TrackId(3), fixture.wav("c.wav", RATE, 2, 0.3))));

    fixture.expect_started(3, true);
    fixture.expect(EngineEvent::EndOfQueue);
}

#[test]
fn stop_unloads_everything() {
    let fixture = Fixture::new();
    fixture.play_from_start(1, fixture.wav("a.wav", RATE, 2, 2.0));
    std::thread::sleep(Duration::from_millis(200));

    fixture.engine.stop();
    fixture.expect(EngineEvent::StateChanged(PlaybackState::Stopped));
    assert_eq!(fixture.engine.position(), Duration::ZERO);

    // Nothing is loaded any more, so play does nothing.
    fixture.engine.play();
    fixture.assert_no_event(Duration::from_millis(200));
    assert_eq!(fixture.engine.state(), PlaybackState::Stopped);
}

#[test]
fn dropping_the_engine_does_not_hang() {
    let fixture = Fixture::new();
    fixture.play_from_start(1, fixture.wav("a.wav", RATE, 2, 5.0));
    std::thread::sleep(Duration::from_millis(100));

    let started = Instant::now();
    drop(fixture.engine);
    assert!(started.elapsed() < Duration::from_secs(1));
}
