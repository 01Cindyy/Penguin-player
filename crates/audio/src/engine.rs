//! The engine thread: owns the decoders and the output, turns commands into audio and
//! reports what happens as [`EngineEvent`]s.
//!
//! Three track slots describe what is going on:
//!
//! - `current`: the track being heard (or loaded, while paused);
//! - `queued`: the next track once its audio is going into the ring buffer, until the
//!   output actually reaches it;
//! - `next`: the next track, opened as soon as it is known but not decoded yet.
//!
//! The thread sleeps until a command arrives or until the next thing it has to do: top up
//! the buffer, notice that a track boundary has been heard, or pause the device once a
//! fade-out has played. Nothing polls in a loop.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use log::{debug, warn};
use mp_core::{PlaybackState, TrackId};

use crate::output::{Output, OutputFormat, Sink};
use crate::playback::Playback;
use crate::shared::Shared;
use crate::{AudioError, EngineConfig, EngineEvent};

/// Shortest sleep between checks, so timing estimates that round down never spin.
const MIN_WAIT: Duration = Duration::from_millis(3);
/// How soon to retry when decoded audio is waiting for room in the ring buffer.
const BACKLOG_RETRY: Duration = Duration::from_millis(10);

pub(crate) enum Command {
    Load {
        track: TrackId,
        path: PathBuf,
        autoplay: bool,
    },
    SetNext(Option<(TrackId, PathBuf)>),
    Play,
    Pause,
    Stop,
    Seek(Duration),
    SetGapless(bool),
    /// The output stream with id `output` failed.
    OutputFailed {
        output: u64,
        message: String,
    },
    Shutdown,
}

/// The next track once the current one has been fully decoded.
struct Queued {
    playback: Playback,
    /// Where its audio starts in the ring; `None` until it starts. That is as soon as audio
    /// is written if it can follow without a gap, otherwise once the output has drained.
    boundary: Option<Boundary>,
}

#[derive(Debug, Clone, Copy)]
struct Boundary {
    serial: u64,
    start: u64,
}

pub(crate) struct Engine {
    commands: Receiver<Command>,
    /// Handed to output streams so they can report failures.
    command_tx: Sender<Command>,
    events: async_channel::Sender<EngineEvent>,
    shared: Arc<Shared>,
    gapless: bool,
    null_output: bool,
    sink: Option<Sink>,
    output: Option<Output>,
    state: PlaybackState,
    current: Option<Playback>,
    queued: Option<Queued>,
    next: Option<Playback>,
    /// The current track restarts from this position the next time audio is written,
    /// dropping everything queued in the output.
    restart: Option<Duration>,
    /// When to pause the output device after a fade-out.
    suspend_at: Option<Instant>,
    /// Source of segment serials and output ids.
    serial: u64,
}

impl Engine {
    pub fn new(
        config: EngineConfig,
        commands: Receiver<Command>,
        command_tx: Sender<Command>,
        events: async_channel::Sender<EngineEvent>,
        shared: Arc<Shared>,
    ) -> Self {
        Self {
            commands,
            command_tx,
            events,
            shared,
            gapless: config.gapless,
            null_output: config.null_output,
            sink: None,
            output: None,
            state: PlaybackState::Stopped,
            current: None,
            queued: None,
            next: None,
            restart: None,
            suspend_at: None,
            serial: 0,
        }
    }

    pub fn run(mut self) {
        loop {
            let command = match self.wait_time() {
                None => self.commands.recv().ok(),
                Some(timeout) => match self.commands.recv_timeout(timeout) {
                    Ok(command) => Some(command),
                    Err(RecvTimeoutError::Timeout) => {
                        self.update();
                        continue;
                    }
                    Err(RecvTimeoutError::Disconnected) => None,
                },
            };
            let Some(command) = command else { break };
            if !self.handle(command) {
                break;
            }
            // Handle everything already waiting before decoding more audio.
            while let Ok(command) = self.commands.try_recv() {
                if !self.handle(command) {
                    return self.shut_down();
                }
            }
            self.update();
        }
        self.shut_down();
    }

    /// Carries out one command. Returns false when the engine should shut down.
    fn handle(&mut self, command: Command) -> bool {
        match command {
            Command::Load {
                track,
                path,
                autoplay,
            } => self.load(track, path, autoplay),
            Command::SetNext(next) => self.set_next(next),
            Command::Play => self.play(),
            Command::Pause => self.pause(),
            Command::Stop => self.stop(),
            Command::Seek(position) => self.seek(position),
            Command::SetGapless(enabled) => self.gapless = enabled,
            Command::OutputFailed { output, message } => {
                // Errors from a stream that has since been replaced are stale.
                if self.output.as_ref().is_some_and(|open| open.id() == output) {
                    self.recover_output(&message);
                }
            }
            Command::Shutdown => return false,
        }
        true
    }

    /// Does whatever is due: reports heard track changes, keeps the output fed, moves on
    /// to the next track and pauses the device after a fade-out.
    fn update(&mut self) {
        self.check_heard();
        self.suspend_if_due();
        if self.state != PlaybackState::Playing {
            return;
        }
        self.apply_restart();
        self.fill();
        self.check_drained();
    }

    // Commands.

    fn load(&mut self, track: TrackId, path: PathBuf, autoplay: bool) {
        self.next = None;
        self.queued = None;
        match Playback::open(track, path) {
            Ok(playback) => {
                let duration = playback.duration();
                self.current = Some(playback);
                self.restart_at(Duration::ZERO);
                self.emit(EngineEvent::TrackStarted {
                    track,
                    duration,
                    from_queue: false,
                });
                if autoplay {
                    self.play();
                } else {
                    self.fade_out();
                    self.set_state(PlaybackState::Paused);
                }
            }
            Err(error) => {
                warn!("cannot play track {}: {error}", track.0);
                self.emit(EngineEvent::TrackFailed {
                    track,
                    message: error.to_string(),
                });
                self.stop();
            }
        }
    }

    fn set_next(&mut self, next: Option<(TrackId, PathBuf)>) {
        self.check_heard();
        let is_next = |playback: &Playback| {
            next.as_ref()
                .is_some_and(|(track, path)| playback.is(*track, path))
        };
        if let Some(queued) = &self.queued {
            if is_next(&queued.playback) {
                return;
            }
            self.cancel_queued();
        } else if self.next.as_ref().is_some_and(is_next) {
            return;
        }

        self.next = next.and_then(|(track, path)| self.open_next(track, path));
    }

    fn play(&mut self) {
        if self.current.is_none() {
            return;
        }
        match self.start_output() {
            Ok(()) => self.set_state(PlaybackState::Playing),
            Err(error) => self.output_failed(error),
        }
    }

    fn pause(&mut self) {
        if self.state == PlaybackState::Playing {
            self.fade_out();
            self.set_state(PlaybackState::Paused);
        }
    }

    /// Stops playback and forgets all tracks.
    fn stop(&mut self) {
        self.current = None;
        self.queued = None;
        self.next = None;
        self.restart = None;
        let serial = self.next_serial();
        self.shared.jump(serial, Duration::ZERO);
        self.fade_out();
        self.set_state(PlaybackState::Stopped);
    }

    fn seek(&mut self, position: Duration) {
        self.check_heard();
        let Some(current) = &mut self.current else {
            return;
        };
        match current.seek(position) {
            Ok(reached) => {
                self.requeue_next();
                self.restart_at(reached);
                self.emit(EngineEvent::Seeked(reached));
            }
            Err(error) => warn!("seek to {position:?} failed: {error}"),
        }
    }

    // Track transitions.

    /// Emits `TrackStarted` once the output has reached the queued track.
    fn check_heard(&mut self) {
        let Some(boundary) = self.queued.as_ref().and_then(|queued| queued.boundary) else {
            return;
        };
        if self.shared.segment() < boundary.serial {
            return;
        }
        if let Some(queued) = self.queued.take() {
            let playback = queued.playback;
            self.emit(EngineEvent::TrackStarted {
                track: playback.track,
                duration: playback.duration(),
                from_queue: true,
            });
            self.current = Some(playback);
        }
    }

    /// Queues the next track to follow the current one, which has no more audio. While a
    /// track is already queued nothing changes: what follows that one is not known yet.
    fn queue_next(&mut self) {
        if self.queued.is_none() {
            self.queued = self.next.take().map(|playback| Queued {
                playback,
                boundary: None,
            });
        }
    }

    /// Starts the queued track right after the audio written so far if it can follow without
    /// a gap. Only called while playing: a paused output takes no markers off its queue, so
    /// changing the next track over and over while paused must not add any.
    fn start_queued_if_seamless(&mut self) {
        let Some(queued) = &self.queued else {
            return;
        };
        if queued.boundary.is_some() || !self.gapless {
            return;
        }
        let format = self.format_for(queued.playback.sample_rate());
        if self
            .output
            .as_ref()
            .is_some_and(|output| Some(output.format()) == format)
        {
            self.start_queued();
        }
    }

    /// Starts the queued track's segment at the end of what has been written.
    fn start_queued(&mut self) {
        let serial = self.next_serial();
        let (Some(queued), Some(output)) = (&mut self.queued, &mut self.output) else {
            return;
        };
        match output.writer.start_segment(serial, Some(Duration::ZERO)) {
            Ok(start) => queued.boundary = Some(Boundary { serial, start }),
            Err(_) => self.recover_output("the output stalled"),
        }
    }

    /// Takes back a queued track whose audio may already be in the ring: the output plays
    /// on seamlessly where that audio would have started.
    fn cancel_queued(&mut self) {
        let Some(boundary) = self.queued.as_ref().and_then(|queued| queued.boundary) else {
            self.queued = None;
            return;
        };
        let serial = self.next_serial();
        let cut = self
            .output
            .as_mut()
            .map(|output| output.writer.cut(boundary.start, serial));
        if let Some(Err(_)) = cut {
            // The next track was replaced many times before playback reached it. Restarting
            // at what is being heard drops all of that audio, at the cost of a short fade. The
            // queued track stays until then: if playback has just reached it, it is the one
            // being heard.
            self.restart_where_heard();
        }
        self.queued = None;
    }

    /// Turns a queued track back into the prepared next track, for when the audio in the
    /// output is about to be dropped (seek, output change).
    fn requeue_next(&mut self) {
        let Some(queued) = self.queued.take() else {
            return;
        };
        let mut playback = queued.playback;
        match playback.seek(Duration::ZERO) {
            Ok(_) => self.next = Some(playback),
            Err(error) => warn!("cannot rewind the next track: {error}"),
        }
    }

    /// Reacts to the output having played everything written: starts a queued track that
    /// needed a fresh output, or ends playback when nothing follows.
    fn check_drained(&mut self) {
        if !self
            .output
            .as_ref()
            .is_some_and(|output| output.writer.is_drained())
        {
            return;
        }
        match &self.queued {
            Some(queued) if queued.boundary.is_none() => self.start_after_drain(),
            Some(_) => {}
            None => {
                let current_done = self.current.as_ref().is_none_or(Playback::is_finished);
                if current_done && self.next.is_none() {
                    self.emit(EngineEvent::EndOfQueue);
                    self.stop();
                }
            }
        }
    }

    /// Starts the queued track once the previous one has fully played, reopening the
    /// output if the new track needs another format.
    fn start_after_drain(&mut self) {
        let Some(rate) = self
            .queued
            .as_ref()
            .map(|queued| queued.playback.sample_rate())
        else {
            return;
        };
        if let Err(error) = self.open_output_for(rate).and_then(|()| self.play_output()) {
            // Nothing can be played: make the queued track current so playing resumes there.
            if let Some(queued) = self.queued.take() {
                self.current = Some(queued.playback);
                self.restart_at(Duration::ZERO);
            }
            self.output_failed(error);
            return;
        }
        self.start_queued();
        self.fill();
    }

    // Feeding the output.

    /// Drops everything queued in the output if a restart is pending (load, seek).
    fn apply_restart(&mut self) {
        let Some(position) = self.restart else {
            return;
        };
        let serial = self.next_serial();
        let Some(output) = &mut self.output else {
            return;
        };
        self.restart = None;
        if output.writer.flush(serial, position).is_err() {
            self.recover_output("the output stalled");
        }
    }

    /// Decodes audio into the output until it holds enough, moving on to the next track when
    /// the current one has no more audio.
    fn fill(&mut self) {
        loop {
            if self.current.as_ref().is_some_and(Playback::is_finished) {
                self.queue_next();
            }
            self.start_queued_if_seamless();
            let Some(output) = &mut self.output else {
                return;
            };
            if !output.writer.push_backlog() || !output.needs_audio() {
                return;
            }
            let source = match (&mut self.queued, &mut self.current) {
                (Some(queued), _) => match queued.boundary {
                    Some(_) => &mut queued.playback,
                    None => return,
                },
                (None, Some(current)) => current,
                (None, None) => return,
            };
            if source.is_finished() {
                return;
            }
            if let Err(error) = source.decode_into(output) {
                let track = source.track;
                warn!("decoding track {} failed: {error}", track.0);
                self.emit(EngineEvent::TrackFailed {
                    track,
                    message: error.to_string(),
                });
            }
        }
    }

    /// The track whose audio is being decoded, if any is left to decode.
    fn decoding(&self) -> Option<&Playback> {
        let source = match &self.queued {
            Some(queued) => queued.boundary.map(|_| &queued.playback),
            None => self.current.as_ref(),
        };
        source.filter(|playback| !playback.is_finished())
    }

    /// How long the thread may sleep before something needs doing; `None` means until the
    /// next command.
    fn wait_time(&self) -> Option<Duration> {
        let mut wait = self
            .suspend_at
            .map(|at| at.saturating_duration_since(Instant::now()));
        let mut wake_within = |duration: Duration| {
            wait = Some(wait.map_or(duration, |current| current.min(duration)));
        };
        if self.state == PlaybackState::Playing
            && let Some(output) = &self.output
        {
            if output.writer.has_backlog() {
                wake_within(BACKLOG_RETRY);
            } else if self.decoding().is_some() {
                wake_within(output.time_until_refill());
            } else {
                wake_within(output.time_until(output.writer.end()));
            }
            if let Some(boundary) = self.queued.as_ref().and_then(|queued| queued.boundary) {
                wake_within(output.time_until(boundary.start));
            }
        }
        wait.map(|wait| wait.max(MIN_WAIT))
    }

    // The output.

    fn format_for(&self, sample_rate: u32) -> Option<OutputFormat> {
        self.sink.as_ref().map(|sink| sink.format_for(sample_rate))
    }

    /// Makes sure an output in the right format for the current track is open, and plays.
    fn start_output(&mut self) -> Result<(), AudioError> {
        let Some(rate) = self.current.as_ref().map(Playback::sample_rate) else {
            return Ok(());
        };
        self.open_output_for(rate)?;
        if self.restart.is_none() && self.output_is_empty() {
            self.restart_where_heard();
        }
        // Drop stale audio (from before a seek or load while paused) before the stream runs.
        self.apply_restart();
        self.play_output()
    }

    /// Opens an output for audio at `sample_rate` unless the open one already suits it.
    fn open_output_for(&mut self, sample_rate: u32) -> Result<(), AudioError> {
        let sink = match &mut self.sink {
            Some(sink) => sink,
            empty => empty.insert(Sink::open(self.null_output)?),
        };
        let format = sink.format_for(sample_rate);
        if self
            .output
            .as_ref()
            .is_some_and(|output| output.format() == format)
        {
            return Ok(());
        }
        if let Some(output) = self.output.take() {
            output.close();
        }
        let id = self.next_serial();
        let commands = self.command_tx.clone();
        let on_error = move |output, message| {
            let _ = commands.send(Command::OutputFailed { output, message });
        };
        let sink = self.sink.as_ref().ok_or(AudioError::NoDevice)?;
        self.output = Some(sink.open_output(format, &self.shared, id, on_error)?);
        Ok(())
    }

    fn output_is_empty(&self) -> bool {
        self.output
            .as_ref()
            .is_some_and(|output| output.writer.end() == 0)
    }

    fn play_output(&mut self) -> Result<(), AudioError> {
        self.suspend_at = None;
        match &mut self.output {
            Some(output) => output.play(),
            None => Err(AudioError::NoDevice),
        }
    }

    /// Fades the output out and schedules the device to pause once that has been heard.
    fn fade_out(&mut self) {
        match &self.output {
            Some(output) => self.suspend_at = Some(Instant::now() + output.fade_out()),
            None => self.shared.set_playing(false),
        }
    }

    fn suspend_if_due(&mut self) {
        if self.suspend_at.is_none_or(|at| Instant::now() < at) {
            return;
        }
        self.suspend_at = None;
        if self.state != PlaybackState::Playing
            && let Some(output) = &mut self.output
        {
            output.suspend();
        }
    }

    /// Restarts the current track from what was last heard, e.g. on a new output.
    fn restart_where_heard(&mut self) {
        self.check_heard();
        let heard = self.shared.position();
        self.requeue_next();
        let Some(current) = &mut self.current else {
            return;
        };
        match current.seek(heard) {
            Ok(reached) => self.restart_at(reached),
            Err(error) => {
                warn!("cannot resume at {heard:?}: {error}");
                self.restart_at(heard);
            }
        }
    }

    /// Replaces a failed output with a new one and carries on where playback was.
    fn recover_output(&mut self, reason: &str) {
        warn!("audio output failed ({reason}), reopening it");
        self.output = None;
        self.sink = None;
        self.suspend_at = None;
        if self.current.is_none() {
            return;
        }
        self.restart_where_heard();
        if self.state == PlaybackState::Playing
            && let Err(error) = self.start_output()
        {
            self.output_failed(error);
        }
    }

    /// Gives up on the output: reports the error and pauses.
    fn output_failed(&mut self, error: AudioError) {
        warn!("{error}");
        self.output = None;
        self.shared.set_playing(false);
        self.emit(EngineEvent::OutputError(error.to_string()));
        if self.current.is_some() {
            self.set_state(PlaybackState::Paused);
        } else {
            self.set_state(PlaybackState::Stopped);
        }
    }

    fn shut_down(mut self) {
        if let Some(output) = self.output.take() {
            output.close();
        }
    }

    // Small helpers.

    fn open_next(&mut self, track: TrackId, path: PathBuf) -> Option<Playback> {
        match Playback::open(track, path) {
            Ok(playback) => Some(playback),
            Err(error) => {
                warn!("cannot play track {}: {error}", track.0);
                self.emit(EngineEvent::TrackFailed {
                    track,
                    message: error.to_string(),
                });
                None
            }
        }
    }

    /// Makes the current track restart from `position`, which `position()` reports at once.
    fn restart_at(&mut self, position: Duration) {
        let serial = self.next_serial();
        self.shared.jump(serial, position);
        self.restart = Some(position);
    }

    fn set_state(&mut self, state: PlaybackState) {
        if self.state != state {
            self.state = state;
            self.shared.set_state(state);
            self.emit(EngineEvent::StateChanged(state));
        }
    }

    fn emit(&self, event: EngineEvent) {
        debug!("{event:?}");
        // The receiver may be gone while the app shuts down; nothing to do then.
        let _ = self.events.try_send(event);
    }

    fn next_serial(&mut self) -> u64 {
        self.serial += 1;
        self.serial
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::mpsc;

    use super::*;
    use crate::test_files;

    const RATE: u32 = 8_000;

    fn null_engine() -> Engine {
        let (command_tx, commands) = mpsc::channel();
        let (events, _) = async_channel::unbounded();
        let config = EngineConfig {
            null_output: true,
            ..EngineConfig::default()
        };
        let shared = Arc::new(Shared::new(1.0));
        Engine::new(config, commands, command_tx, events, shared)
    }

    /// Writes `count` WAV files shorter than the decode-ahead window, so each is decoded in
    /// one go and the next track's audio follows it in the output straight away.
    fn short_tracks(dir: &Path, count: usize) -> Vec<PathBuf> {
        (0..count)
            .map(|index| {
                let path = dir.join(format!("{index}.wav"));
                test_files::write_wav(&path, RATE, 2, RATE * 3 / 10, test_files::tone(RATE));
                path
            })
            .collect()
    }

    /// Plays track 0 with track 1 queued after it.
    fn play_with_next(engine: &mut Engine, tracks: &[PathBuf]) {
        engine.handle(Command::Load {
            track: TrackId(0),
            path: tracks[0].clone(),
            autoplay: true,
        });
        engine.handle(Command::SetNext(Some((TrackId(1), tracks[1].clone()))));
        engine.update();
    }

    /// Alternates the next track between tracks 1 and 2, `times` times, ending with 2.
    fn change_next_repeatedly(engine: &mut Engine, tracks: &[PathBuf], times: usize) {
        for index in 0..times {
            let next = if index % 2 == 0 { 1 } else { 2 };
            let path = tracks[next].clone();
            engine.handle(Command::SetNext(Some((TrackId(next as i64), path))));
            engine.update();
        }
    }

    fn output_id(engine: &Engine) -> Option<u64> {
        engine.output.as_ref().map(Output::id)
    }

    #[test]
    fn changing_the_next_track_while_paused_keeps_the_output() {
        let dir = tempfile::tempdir().expect("temp dir");
        let tracks = short_tracks(dir.path(), 3);
        let mut engine = null_engine();
        play_with_next(&mut engine, &tracks);
        engine.handle(Command::Pause);
        engine.update();
        let output = output_id(&engine);

        // The paused output takes no markers off its queue, so the queue must not grow.
        change_next_repeatedly(&mut engine, &tracks, 100);
        assert!(output.is_some());
        assert_eq!(output_id(&engine), output);
    }

    #[test]
    fn changing_the_next_track_many_times_while_playing_keeps_the_output() {
        let dir = tempfile::tempdir().expect("temp dir");
        let tracks = short_tracks(dir.path(), 3);
        let mut engine = null_engine();
        play_with_next(&mut engine, &tracks);
        let output = output_id(&engine);

        // Every change replaces audio already in the output, and the output only takes the
        // markers for that off its queue once it gets there.
        change_next_repeatedly(&mut engine, &tracks, 100);
        assert!(output.is_some());
        assert_eq!(output_id(&engine), output, "the output was reopened");
        let following = match &engine.queued {
            Some(queued) => Some(queued.playback.track),
            None => engine.next.as_ref().map(|next| next.track),
        };
        assert_eq!(
            following,
            Some(TrackId(2)),
            "the latest next track is not queued"
        );
    }

    #[test]
    fn a_queued_track_reached_as_it_is_cancelled_without_room_for_a_cut_is_kept() {
        let dir = tempfile::tempdir().expect("temp dir");
        let tracks = short_tracks(dir.path(), 3);
        let mut engine = null_engine();
        play_with_next(&mut engine, &tracks);
        // A suspended output takes no markers off its queue, so the cuts pile up.
        engine.output.as_mut().expect("open output").suspend();
        let has_room_for_cut = |engine: &Engine| {
            let output = engine.output.as_ref().expect("open output");
            output.writer.has_room_for_cut()
        };
        for index in 0..1_000 {
            if !has_room_for_cut(&engine) {
                break;
            }
            let next = 2 - index % 2;
            let path = tracks[next].clone();
            engine.handle(Command::SetNext(Some((TrackId(next as i64), path))));
            engine.update();
        }
        assert!(!has_room_for_cut(&engine), "cuts never ran out of room");
        let queued = engine.queued.as_ref().expect("queued track");
        let (track, boundary) = (queued.playback.track, queued.boundary.expect("started"));

        // Playback reaches the queued track in the instant between `set_next` checking for
        // that and cancelling it.
        engine.shared.publish_segment(boundary.serial, 0);
        engine.cancel_queued();

        let current = engine.current.as_ref().map(|current| current.track);
        assert_eq!(current, Some(track), "the track being heard was dropped");
        assert_eq!(engine.restart, Some(Duration::ZERO));
    }

    #[test]
    fn a_track_that_cannot_resume_on_a_new_output_is_followed_by_the_next() {
        let dir = tempfile::tempdir().expect("temp dir");
        let tracks = short_tracks(dir.path(), 2);
        let mut engine = null_engine();
        play_with_next(&mut engine, &tracks);

        // The first track has been read to its end, so resuming it means reopening the file.
        std::fs::remove_file(&tracks[0]).expect("delete");
        let output = output_id(&engine).expect("open output");
        engine.handle(Command::OutputFailed {
            output,
            message: "unplugged".into(),
        });
        engine.update();

        assert_eq!(engine.state, PlaybackState::Playing);
        let next_started = engine
            .queued
            .as_ref()
            .is_some_and(|queued| queued.boundary.is_some());
        assert!(next_started, "the next track was not started");
    }
}
