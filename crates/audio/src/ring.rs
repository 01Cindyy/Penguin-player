//! The lock-free pipe between the engine thread and the output callback.
//!
//! Decoded samples (interleaved `f32` in the output's format) flow through an SPSC ring
//! buffer. Positions in that sample stream are absolute sample indices counted from the
//! creation of the ring, and two kinds of marker refer to them:
//!
//! - a [`Segment`] says "from sample N on, this is track position P of segment S". The
//!   engine starts one for every track and every seek. The reader publishes which segment
//!   is playing, which is how the engine knows when a gapless transition is actually heard.
//! - a [`Cut`] seamlessly drops queued audio from sample N up to the start of a later
//!   segment. It replaces a track that was queued for gapless playback.
//!
//! A *flush* (load, seek) fades out whatever is playing and drops everything queued
//! before a given segment. It is signalled through an atomic rather than a queue so it
//! never waits behind a cut that has not been reached yet.

use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use rtrb::{Consumer, Producer, RingBuffer};

use crate::shared::{Shared, duration_to_nanos, frames_to_nanos};

/// Length of the fades used for pausing, seeking and volume changes.
pub(crate) const RAMP: Duration = Duration::from_millis(15);
/// Capacity of the marker queues. Markers wait there until playback reaches them, so they
/// only pile up when the next track is replaced again and again before it starts.
const MARKER_CAPACITY: usize = 256;
/// Bounds the work of one `render` call. Each pass either produces audio or handles a
/// marker; the cap only protects the audio thread from a logic error.
const MAX_PASSES: usize = 2 * MARKER_CAPACITY;

#[derive(Debug, Clone, Copy)]
struct Segment {
    /// Sample index where the segment starts.
    start: u64,
    serial: u64,
    /// Track position at `start`; `None` carries on from the previous segment's position.
    position_ns: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
struct Cut {
    /// Playback continues up to this sample index...
    from: u64,
    /// ...then jumps to the start of this segment.
    to_serial: u64,
}

/// Counters both ends of one ring share.
#[derive(Default)]
struct Counters {
    /// Samples taken out of the ring by the reader, played or dropped.
    consumed: AtomicU64,
    /// Serial of the segment the latest flush jumps to.
    flush_to: AtomicU64,
}

/// There was no room for another marker: the output has not run for a long time, or the
/// next track was replaced many times before playback reached it.
#[derive(Debug)]
pub(crate) struct MarkersFull;

pub(crate) fn ring(
    capacity_samples: usize,
    channels: usize,
    sample_rate: u32,
    shared: Arc<Shared>,
) -> (RingWriter, RingReader) {
    let (samples_tx, samples_rx) = RingBuffer::new(capacity_samples);
    let (segments_tx, segments_rx) = RingBuffer::new(MARKER_CAPACITY);
    let (cuts_tx, cuts_rx) = RingBuffer::new(MARKER_CAPACITY);
    let counters = Arc::new(Counters::default());
    let writer = RingWriter {
        samples: samples_tx,
        segments: segments_tx,
        cuts: cuts_tx,
        counters: Arc::clone(&counters),
        written: 0,
        backlog: Vec::new(),
        doomed: 0..0,
    };
    let sample_rate = sample_rate.max(1);
    let reader = RingReader {
        samples: samples_rx,
        segments: segments_rx,
        cuts: cuts_rx,
        counters,
        shared,
        channels,
        sample_rate: u64::from(sample_rate),
        ramp_step: 1.0 / (RAMP.as_secs_f32() * sample_rate as f32),
        read: 0,
        gain: 0.0,
        flushed_to: 0,
        serial: 0,
        segment_position_ns: 0,
        segment_frames: 0,
    };
    (writer, reader)
}

/// The engine thread's end of the ring.
pub(crate) struct RingWriter {
    samples: Producer<f32>,
    segments: Producer<Segment>,
    cuts: Producer<Cut>,
    counters: Arc<Counters>,
    /// Samples pushed into the ring so far.
    written: u64,
    /// Samples waiting for room in the ring. They logically follow `written`.
    backlog: Vec<f32>,
    /// Sample indices that will be dropped rather than played.
    doomed: Range<u64>,
}

impl RingWriter {
    /// Queues whole frames of audio. What does not fit waits in a backlog.
    pub fn write(&mut self, samples: &[f32]) {
        if self.push_backlog() {
            let (pushed, rest) = self.samples.push_partial_slice(samples);
            self.written += pushed.len() as u64;
            self.backlog.extend_from_slice(rest);
        } else {
            self.backlog.extend_from_slice(samples);
        }
    }

    /// Moves backlogged samples into the ring. Returns whether the backlog is now empty.
    pub fn push_backlog(&mut self) -> bool {
        if !self.backlog.is_empty() {
            let (pushed, _) = self.samples.push_partial_slice(&self.backlog);
            let count = pushed.len();
            self.written += count as u64;
            self.backlog.drain(..count);
        }
        self.backlog.is_empty()
    }

    pub fn has_backlog(&self) -> bool {
        !self.backlog.is_empty()
    }

    /// Index of the next sample to be written.
    pub fn end(&self) -> u64 {
        self.written + self.backlog.len() as u64
    }

    pub fn consumed(&self) -> u64 {
        self.counters.consumed.load(Ordering::Acquire)
    }

    /// Samples waiting to be played, not counting those that will be dropped.
    pub fn queued(&self) -> u64 {
        let consumed = self.consumed();
        let doomed = self
            .doomed
            .end
            .saturating_sub(self.doomed.start.max(consumed));
        self.end().saturating_sub(consumed).saturating_sub(doomed)
    }

    /// Everything written has been played or dropped.
    pub fn is_drained(&self) -> bool {
        self.consumed() >= self.end()
    }

    /// Starts a segment at the current end of the stream and returns its start index.
    pub fn start_segment(
        &mut self,
        serial: u64,
        position: Option<Duration>,
    ) -> Result<u64, MarkersFull> {
        let start = self.end();
        let segment = Segment {
            start,
            serial,
            position_ns: position.map(duration_to_nanos),
        };
        self.segments.push(segment).map_err(|_| MarkersFull)?;
        Ok(start)
    }

    /// Fades out whatever is playing, drops all queued audio and continues with a new
    /// segment at `position`.
    pub fn flush(&mut self, serial: u64, position: Duration) -> Result<(), MarkersFull> {
        self.backlog.clear();
        let start = self.start_segment(serial, Some(position))?;
        self.mark_doomed(0..start);
        self.counters.flush_to.store(serial, Ordering::Release);
        Ok(())
    }

    /// Whether [`RingWriter::cut`] would accept a cut. Cuts may fill only half of each marker
    /// queue, so a flush always fits after them.
    pub fn has_room_for_cut(&self) -> bool {
        let half = MARKER_CAPACITY / 2;
        self.segments.slots() > half && self.cuts.slots() > half
    }

    /// Drops the queued audio from sample `from` on, seamlessly: playback reaches `from` and
    /// carries on with whatever is written next, at the same track position. A refused cut
    /// changes nothing.
    pub fn cut(&mut self, from: u64, serial: u64) -> Result<(), MarkersFull> {
        if !self.has_room_for_cut() {
            return Err(MarkersFull);
        }
        let kept = usize::try_from(from.saturating_sub(self.written)).unwrap_or(usize::MAX);
        self.backlog.truncate(kept);
        let start = self.start_segment(serial, None)?;
        self.mark_doomed(from..start);
        self.cuts
            .push(Cut {
                from,
                to_serial: serial,
            })
            .map_err(|_| MarkersFull)
    }

    /// Records `range` as dropped for [`RingWriter::queued`]. Only one range is tracked; an
    /// older one that is still pending and not adjacent is forgotten, which merely makes the
    /// fill level look a little higher than it is for a moment.
    fn mark_doomed(&mut self, range: Range<u64>) {
        let consumed = self.consumed();
        self.doomed = if self.doomed.end > consumed && self.doomed.end >= range.start {
            self.doomed.start.min(range.start)..range.end.max(self.doomed.end)
        } else {
            range
        };
    }
}

/// The output callback's end of the ring.
///
/// Runs on the audio device's thread: it never allocates, locks, logs or blocks. When
/// there is no audio it outputs silence.
pub(crate) struct RingReader {
    samples: Consumer<f32>,
    segments: Consumer<Segment>,
    cuts: Consumer<Cut>,
    counters: Arc<Counters>,
    shared: Arc<Shared>,
    channels: usize,
    sample_rate: u64,
    /// Gain change per frame during a fade.
    ramp_step: f32,
    /// Index of the next sample to read.
    read: u64,
    gain: f32,
    /// Serial of the last flush carried out.
    flushed_to: u64,
    /// Serial of the segment being played.
    serial: u64,
    segment_position_ns: u64,
    /// Frames played since the current segment started.
    segment_frames: u64,
}

impl RingReader {
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Fills `out` (interleaved, a whole number of frames) with the next audio.
    pub fn render(&mut self, out: &mut [f32]) {
        let mut filled = 0;
        for _ in 0..MAX_PASSES {
            if filled == out.len() {
                break;
            }
            // Count the audio before looking at the markers. The writer queues a marker before
            // the audio it applies to, so the markers for all of this audio are visible below;
            // audio that arrives later may belong to a marker that is not.
            let available = self.samples.slots();
            if self.apply_flush() || self.apply_cut() {
                continue;
            }
            self.enter_reached_segments();

            let fading_out = self.must_fade_out();
            if fading_out && self.gain == 0.0 {
                break;
            }
            let mut wanted = (out.len() - filled).min(self.samples_until_marker());
            if fading_out {
                wanted = wanted.min(self.samples_until_silent());
            }
            let target = if fading_out {
                0.0
            } else {
                self.shared.volume()
            };
            let read = self.read_into(&mut out[filled..filled + wanted], available, target);
            filled += read;
            if read < wanted {
                // Underrun: play silence, and fade the audio back in when it returns.
                self.gain = 0.0;
                break;
            }
        }
        out[filled..].fill(0.0);
        self.shared
            .scope
            .push(out, self.channels, self.sample_rate as u32);

        let silent = self.must_fade_out() && self.gain == 0.0;
        self.counters.consumed.store(self.read, Ordering::Release);
        self.shared.publish_progress(self.position_ns(), silent);
    }

    fn flush_pending(&self) -> bool {
        self.counters.flush_to.load(Ordering::Acquire) > self.flushed_to
    }

    /// Paused, flushing, or past a cut point: fade out and stop taking audio.
    fn must_fade_out(&self) -> bool {
        !self.shared.is_playing()
            || self.flush_pending()
            || self.cuts.peek().is_ok_and(|cut| cut.from < self.read)
    }

    /// Carries out a pending flush once the fade-out has finished.
    fn apply_flush(&mut self) -> bool {
        let target = self.counters.flush_to.load(Ordering::Acquire);
        if target <= self.flushed_to || self.gain > 0.0 {
            return false;
        }
        self.flushed_to = target;
        // Cuts issued before the flush refer to audio that is being dropped anyway.
        while self.cuts.peek().is_ok_and(|cut| cut.to_serial < target) {
            let _ = self.cuts.pop();
        }
        self.jump_to_segment(target);
        true
    }

    /// Carries out the oldest cut once playback has reached it.
    fn apply_cut(&mut self) -> bool {
        let Ok(&cut) = self.cuts.peek() else {
            return false;
        };
        // Normally the cut point is reached exactly. If the cut came too late and that part
        // is already playing, it fades out first.
        let ready = cut.from == self.read || (cut.from < self.read && self.gain == 0.0);
        if !ready {
            return false;
        }
        let _ = self.cuts.pop();
        self.jump_to_segment(cut.to_serial);
        true
    }

    /// Drops markers and samples up to the start of segment `serial` and enters it.
    fn jump_to_segment(&mut self, serial: u64) {
        // With less than a fade's worth of old audio queued, playback can reach the segment
        // during the fade-out. Jumping again would skip whatever follows it.
        if self.serial >= serial {
            return;
        }
        while let Ok(segment) = self.segments.pop() {
            if segment.serial >= serial {
                self.discard(segment.start.saturating_sub(self.read));
                self.enter(segment);
                break;
            }
        }
    }

    fn enter_reached_segments(&mut self) {
        while let Ok(&segment) = self.segments.peek() {
            if segment.start > self.read {
                break;
            }
            let _ = self.segments.pop();
            self.enter(segment);
        }
    }

    fn enter(&mut self, segment: Segment) {
        self.serial = segment.serial;
        self.segment_position_ns = segment.position_ns.unwrap_or_else(|| self.position_ns());
        self.segment_frames = 0;
        self.shared
            .publish_segment(segment.serial, self.segment_position_ns);
    }

    /// Samples that may be read before the next segment start or cut point.
    fn samples_until_marker(&self) -> usize {
        let segment = self
            .segments
            .peek()
            .map_or(u64::MAX, |segment| segment.start.saturating_sub(self.read));
        let cut = match self.cuts.peek() {
            Ok(cut) if cut.from > self.read => cut.from - self.read,
            _ => u64::MAX,
        };
        usize::try_from(segment.min(cut)).unwrap_or(usize::MAX)
    }

    /// Samples the current fade-out still needs to reach silence.
    fn samples_until_silent(&self) -> usize {
        let frames = (self.gain / self.ramp_step).ceil() as usize;
        frames.max(1) * self.channels
    }

    /// Copies up to `out.len()` of the `available` samples into `out`, applying the gain ramp
    /// towards `target`. Returns how many samples were read.
    fn read_into(&mut self, out: &mut [f32], available: usize, target: f32) -> usize {
        let readable = out.len().min(available);
        let count = readable - readable % self.channels;
        let Ok(chunk) = self.samples.read_chunk(count) else {
            return 0;
        };
        let (first, second) = chunk.as_slices();
        out[..first.len()].copy_from_slice(first);
        out[first.len()..count].copy_from_slice(second);
        chunk.commit_all();

        self.apply_gain(&mut out[..count], target);
        self.read += count as u64;
        self.segment_frames += (count / self.channels) as u64;
        count
    }

    fn apply_gain(&mut self, samples: &mut [f32], target: f32) {
        if self.gain == target {
            // At full volume the samples pass through untouched (bit-transparent).
            if target != 1.0 {
                samples.iter_mut().for_each(|sample| *sample *= target);
            }
            return;
        }
        for frame in samples.chunks_exact_mut(self.channels) {
            self.gain = if self.gain < target {
                (self.gain + self.ramp_step).min(target)
            } else {
                (self.gain - self.ramp_step).max(target)
            };
            frame.iter_mut().for_each(|sample| *sample *= self.gain);
        }
    }

    fn discard(&mut self, count: u64) {
        let count = usize::try_from(count)
            .unwrap_or(usize::MAX)
            .min(self.samples.slots());
        if let Ok(chunk) = self.samples.read_chunk(count) {
            chunk.commit_all();
            self.read += count as u64;
        }
    }

    fn position_ns(&self) -> u64 {
        let played = frames_to_nanos(self.segment_frames, self.sample_rate);
        self.segment_position_ns.saturating_add(played)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 1000;

    fn setup() -> (Arc<Shared>, RingWriter, RingReader) {
        let shared = Arc::new(Shared::new(1.0));
        let (writer, reader) = ring(4000, 2, RATE, Arc::clone(&shared));
        shared.set_playing(true);
        (shared, writer, reader)
    }

    fn render(reader: &mut RingReader, frames: usize) -> Vec<f32> {
        let mut out = vec![9.0; frames * 2];
        reader.render(&mut out);
        out
    }

    #[test]
    fn plays_segments_and_reports_position() {
        let (shared, mut writer, mut reader) = setup();
        writer.start_segment(1, Some(Duration::ZERO)).unwrap();
        writer.write(&[0.5; 200]);
        writer.start_segment(2, Some(Duration::ZERO)).unwrap();
        writer.write(&[0.25; 200]);

        render(&mut reader, 50);
        assert_eq!(shared.segment(), 1);
        assert_eq!(shared.position(), Duration::from_millis(50));

        let out = render(&mut reader, 100);
        assert_eq!(shared.segment(), 2);
        assert_eq!(shared.position(), Duration::from_millis(50));
        // The boundary is seamless: no fade between the two tracks.
        assert_eq!(out[98], 0.5);
        assert_eq!(out[100], 0.25);
        assert_eq!(writer.consumed(), 300);
    }

    #[test]
    fn position_stays_exact_far_into_a_segment() {
        let (_writer, mut reader) = ring(16, 2, 768_000, Arc::new(Shared::new(1.0)));
        // A day of audio at the highest sample rate the decoder accepts.
        reader.segment_frames = 768_000 * 86_400;
        assert_eq!(reader.position_ns(), 86_400 * 1_000_000_000);
    }

    #[test]
    fn underrun_outputs_silence_and_drains() {
        let (_shared, mut writer, mut reader) = setup();
        writer.start_segment(1, Some(Duration::ZERO)).unwrap();
        writer.write(&[1.0; 20]);
        let out = render(&mut reader, 20);
        assert!(out[20..].iter().all(|&sample| sample == 0.0));
        assert!(writer.is_drained());
        assert_eq!(writer.consumed(), 20);
    }

    #[test]
    fn flush_fades_then_jumps() {
        let (shared, mut writer, mut reader) = setup();
        writer.start_segment(1, Some(Duration::ZERO)).unwrap();
        writer.write(&[1.0; 2000]);
        render(&mut reader, 100);

        writer
            .flush(2, Duration::from_secs(30))
            .expect("room for markers");
        writer.write(&[0.5; 2000]);
        assert_eq!(writer.queued(), 2000);

        let out = render(&mut reader, 200);
        assert_eq!(shared.segment(), 2);
        // A short fade-out of the old audio, then the new audio fading in.
        assert!(out[0] < 1.0 && out[0] > 0.9);
        assert!(out.iter().all(|&sample| sample <= 1.0));
        assert_eq!(out[out.len() - 1], 0.5);
        assert!(shared.position() > Duration::from_secs(30));
    }

    #[test]
    fn flush_overtakes_a_pending_cut() {
        let (shared, mut writer, mut reader) = setup();
        writer.start_segment(1, Some(Duration::ZERO)).unwrap();
        writer.write(&[1.0; 1000]);
        render(&mut reader, 50);
        let next_track = writer.start_segment(2, Some(Duration::ZERO)).unwrap();
        writer.write(&[0.25; 100]);
        writer.cut(next_track, 3).expect("room for markers");

        writer
            .flush(4, Duration::from_secs(5))
            .expect("room for markers");
        writer.write(&[0.5; 400]);

        // The flush happens after a short fade, not once the cut point is reached.
        let out = render(&mut reader, 100);
        assert_eq!(shared.segment(), 4);
        assert_eq!(out[2 * 30], 0.5);
        assert!(!out.contains(&0.25));
    }

    #[test]
    fn flush_with_less_than_a_fade_left_keeps_the_new_segment() {
        let (shared, mut writer, mut reader) = setup();
        writer.start_segment(1, Some(Duration::ZERO)).unwrap();
        writer.write(&[1.0; 200]);
        render(&mut reader, 95);
        // 5 frames of old audio left, playing at full gain: the fade-out runs into the new
        // segment, which must still play before the next one.
        writer.flush(2, Duration::from_secs(30)).unwrap();
        writer.write(&[0.5; 200]);
        writer.start_segment(3, Some(Duration::ZERO)).unwrap();
        writer.write(&[0.25; 200]);

        let out = render(&mut reader, 100);
        assert_eq!(shared.segment(), 2);
        assert_eq!(out[out.len() - 1], 0.5);
        assert!(!out.contains(&0.25));
    }

    #[test]
    fn cut_replaces_queued_audio_seamlessly() {
        let (shared, mut writer, mut reader) = setup();
        writer.start_segment(1, Some(Duration::ZERO)).unwrap();
        writer.write(&[1.0; 100]);
        let next_track = writer.start_segment(2, Some(Duration::ZERO)).unwrap();
        writer.write(&[0.25; 100]);

        writer.cut(next_track, 3).expect("room for markers");
        writer.start_segment(4, Some(Duration::ZERO)).unwrap();
        writer.write(&[0.5; 100]);

        let out = render(&mut reader, 100);
        assert_eq!(out[98], 1.0);
        assert_eq!(out[100], 0.5);
        assert_eq!(shared.segment(), 4);
        assert!(!out.contains(&0.25));
    }

    #[test]
    fn cuts_leave_room_for_a_flush() {
        let (shared, mut writer, mut reader) = setup();
        writer.start_segment(1, Some(Duration::ZERO)).unwrap();
        writer.write(&[1.0; 100]);
        // The track after this one is replaced over and over before playback gets there.
        let mut serial = 1;
        loop {
            let next_track = writer
                .start_segment(serial + 1, Some(Duration::ZERO))
                .unwrap();
            writer.write(&[0.25; 10]);
            serial += 2;
            if writer.cut(next_track, serial).is_err() {
                break;
            }
        }

        writer
            .flush(serial + 1, Duration::from_secs(7))
            .expect("room for the flush");
        writer.write(&[0.5; 200]);
        let out = render(&mut reader, 100);
        assert_eq!(shared.segment(), serial + 1);
        assert_eq!(out[out.len() - 1], 0.5);
    }

    #[test]
    fn pause_fades_out_and_stops_consuming() {
        let (shared, mut writer, mut reader) = setup();
        writer.start_segment(1, Some(Duration::ZERO)).unwrap();
        writer.write(&[1.0; 2000]);
        render(&mut reader, 100);
        shared.set_playing(false);
        render(&mut reader, 100);
        assert!(shared.is_silent());
        let consumed = writer.consumed();
        let out = render(&mut reader, 100);
        assert!(out.iter().all(|&sample| sample == 0.0));
        assert_eq!(writer.consumed(), consumed);
    }

    #[test]
    fn volume_scales_samples() {
        let (shared, mut writer, mut reader) = setup();
        shared.set_volume(0.5);
        writer.start_segment(1, Some(Duration::ZERO)).unwrap();
        writer.write(&[1.0; 400]);
        let out = render(&mut reader, 200);
        // Fades in from silence over the ramp, then holds the volume.
        assert!(out[0] < 0.1);
        assert_eq!(out[out.len() - 1], 0.5);
    }

    #[test]
    fn enters_each_segment_before_playing_its_audio() {
        // The writer starts a one-frame segment whenever the reader has played everything,
        // racing the reader's next callback. A segment entered late would count two frames.
        const SEGMENTS: u64 = 500_000;
        let shared = Arc::new(Shared::new(1.0));
        let (mut writer, mut reader) = ring(64, 1, RATE, Arc::clone(&shared));
        shared.set_playing(true);
        let writer = std::thread::spawn(move || {
            for serial in 1..=SEGMENTS {
                while !writer.is_drained() {
                    std::hint::spin_loop();
                }
                writer
                    .start_segment(serial, Some(Duration::ZERO))
                    .expect("the reader takes every marker");
                writer.write(&[0.5]);
            }
        });
        let one_frame = Duration::from_millis(1);
        while !writer.is_finished() || shared.segment() < SEGMENTS {
            reader.render(&mut [0.0]);
            let segment = shared.segment();
            assert!(
                shared.position() <= one_frame,
                "segment {segment} entered late"
            );
        }
        writer.join().expect("writer thread");
    }
}
