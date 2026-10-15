//! Opening and decoding audio files with Symphonia.

use std::collections::VecDeque;
use std::fs::File;
use std::io;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use log::warn;
use symphonia::core::audio::{Channels, GenericAudioBufferRef};
use symphonia::core::codecs::audio::well_known::{
    CODEC_ID_AAC, CODEC_ID_MP1, CODEC_ID_MP2, CODEC_ID_MP3, CODEC_ID_OPUS, CODEC_ID_VORBIS,
};
use symphonia::core::codecs::audio::{AudioCodecId, AudioDecoder, AudioDecoderOptions};
use symphonia::core::codecs::registry::CodecRegistry;
use symphonia::core::errors::{Error as SymphoniaError, SeekErrorKind};
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::well_known::{FORMAT_ID_ISOMP4, FORMAT_ID_MKV};
use symphonia::core::formats::{
    FormatOptions, FormatReader, MediaInfo, SeekMode, SeekTo, Track, TrackType,
};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::packet::Packet;
use symphonia::core::units::{self, Time, TimeBase, Timestamp};

use crate::mp4;

/// After this many undecodable packets in a row the file is treated as broken.
const MAX_CONSECUTIVE_ERRORS: u32 = 256;
/// Sample rates music is plausibly recorded at. A rate outside this range comes from a
/// corrupt header, and playing it would make the output and resampler buffers enormous.
const SAMPLE_RATES: RangeInclusive<u32> = 1_000..=768_000;

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("unsupported format ({0})")]
    Unsupported(String),
    #[error("no playable audio track")]
    NoAudioTrack,
    #[error("malformed audio data ({0})")]
    Malformed(String),
}

impl From<SymphoniaError> for DecodeError {
    fn from(error: SymphoniaError) -> Self {
        match error {
            SymphoniaError::IoError(error) => DecodeError::Io(error),
            SymphoniaError::Unsupported(what) => DecodeError::Unsupported(what.to_owned()),
            other => DecodeError::Malformed(other.to_string()),
        }
    }
}

/// Properties of the audio stream in a file.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamInfo {
    /// Short codec name such as `"flac"` or `"opus"`.
    pub codec: &'static str,
    pub sample_rate: u32,
    pub channels: u16,
    pub duration: Option<Duration>,
}

/// Decodes one audio file into interleaved `f32` samples.
///
/// Encoder delay and padding are trimmed (gapless), seeking is sample accurate, and corrupt
/// packets are skipped. Matroska is the exception: its timestamps are whole milliseconds,
/// so seeks there land within a millisecond or two, and Symphonia does not report its end
/// padding.
pub struct TrackDecoder {
    path: PathBuf,
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    time_base: TimeBase,
    playable: Playable,
    layout: Channels,
    info: StreamInfo,
    samples: Vec<f32>,
    /// Decoded frames before this timestamp are dropped (encoder delay, accurate seeking).
    skip_until: Option<i64>,
    /// Timestamp of the first packet.
    first_ts: i64,
    /// How far before a seek target decoding has to start, in track timestamps.
    seek_preroll: i64,
    /// Seek by reading forward from the start instead of asking the format reader.
    seek_by_scanning: bool,
    /// Packets read ahead while seeking, decoded before reading more.
    pending: VecDeque<Packet>,
    finished: bool,
    /// The format reader has reached the end of the file. Several of Symphonia's readers
    /// (MP4, CAF, Matroska) cannot read again after that, even after seeking.
    reader_at_end: bool,
    frames_decoded: u64,
    consecutive_errors: u32,
    skipped_packets: u64,
}

/// The audible part of a track in track timestamps: `start` is position zero, and frames
/// from `end` on are encoder padding.
#[derive(Debug, Clone, Copy)]
struct Playable {
    start: i64,
    end: Option<i64>,
}

impl TrackDecoder {
    pub fn open(path: &Path) -> Result<Self, DecodeError> {
        let file = File::open(path)?;
        let stream = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        if let Some(extension) = path.extension().and_then(|ext| ext.to_str()) {
            hint.with_extension(extension);
        }
        let format = symphonia::default::get_probe().probe(
            &hint,
            stream,
            FormatOptions::default(),
            MetadataOptions::default(),
        )?;
        Self::from_format(format, path)
    }

    fn from_format(format: Box<dyn FormatReader>, path: &Path) -> Result<Self, DecodeError> {
        let track = format
            .default_track(TrackType::Audio)
            .ok_or(DecodeError::NoAudioTrack)?
            .clone();
        let params = track
            .codec_params
            .as_ref()
            .and_then(|params| params.audio())
            .ok_or(DecodeError::NoAudioTrack)?;
        let decoder = codecs().make_audio_decoder(params, &AudioDecoderOptions::default())?;

        // The codec knows best what it outputs (Opus is always 48 kHz whatever the container
        // says); the container's values are the fallback.
        let decoded_params = decoder.codec_params();
        let sample_rate = decoded_params
            .sample_rate
            .or(params.sample_rate)
            .ok_or_else(|| DecodeError::Unsupported("unknown sample rate".into()))?;
        if !SAMPLE_RATES.contains(&sample_rate) {
            return Err(DecodeError::Unsupported(format!(
                "sample rate of {sample_rate} Hz"
            )));
        }
        let layout = decoded_params
            .channels
            .clone()
            .or_else(|| params.channels.clone())
            .filter(|channels| channels.count() > 0)
            .ok_or_else(|| DecodeError::Unsupported("unknown channel layout".into()))?;
        let channels = u16::try_from(layout.count())
            .map_err(|_| DecodeError::Unsupported("too many channels".into()))?;

        let time_base = track
            .time_base
            .or_else(|| TimeBase::try_from_recip(sample_rate))
            .ok_or_else(|| DecodeError::Unsupported("unknown time base".into()))?;
        let format_id = format.format_info().format;
        let edit_list = (format_id == FORMAT_ID_ISOMP4)
            .then(|| mp4::read_edit_list(path, track.id, time_base.denom.get()))
            .flatten();
        // Symphonia's Matroska reader misreads cue times, so after seeking to any cue point
        // past the first its timestamps are wrong, and without cues it can only seek forwards.
        let seek_by_scanning = format_id == FORMAT_ID_MKV;
        let playable = playable_range(&track, params.codec == CODEC_ID_OPUS, edit_list);
        let preroll = frames_to_duration(seek_preroll_frames(params.codec), sample_rate);
        let duration = track_duration(&track, sample_rate, time_base, playable)
            .or_else(|| media_duration(format.media_info()));

        Ok(Self {
            path: path.to_path_buf(),
            info: StreamInfo {
                codec: decoder.codec_info().short_name,
                sample_rate,
                channels,
                duration,
            },
            format,
            decoder,
            track_id: track.id,
            time_base,
            playable,
            layout,
            samples: Vec::new(),
            skip_until: Some(playable.start),
            first_ts: track.start_ts.get(),
            seek_preroll: duration_to_timestamp(time_base, preroll),
            seek_by_scanning,
            pending: VecDeque::new(),
            finished: false,
            reader_at_end: false,
            frames_decoded: 0,
            consecutive_errors: 0,
            skipped_packets: 0,
        })
    }

    pub fn info(&self) -> &StreamInfo {
        &self.info
    }

    /// The speaker layout of the decoded channels, used for downmixing.
    pub(crate) fn layout(&self) -> &Channels {
        &self.layout
    }

    /// Decodes the next block of audio. Returns `None` at the end of the track.
    pub fn next_chunk(&mut self) -> Result<Option<&[f32]>, DecodeError> {
        while !self.finished {
            let Some(packet) = self.next_packet()? else {
                self.finished = true;
                break;
            };
            let first = packet
                .pts
                .get()
                .saturating_add(i64::try_from(packet.trim_start.get()).unwrap_or(i64::MAX));

            match self.decoder.decode(&packet) {
                Ok(buffer) => {
                    self.consecutive_errors = 0;
                    copy_interleaved(&buffer, usize::from(self.info.channels), &mut self.samples);
                }
                Err(SymphoniaError::DecodeError(_) | SymphoniaError::IoError(_)) => {
                    self.note_bad_packet()?;
                    continue;
                }
                // The decoder adapted to a stream change; the next packets decode normally.
                Err(SymphoniaError::ResetRequired) => continue,
                Err(error) => return Err(error.into()),
            }

            self.trim_to_playable(first);
            if !self.samples.is_empty() {
                self.frames_decoded +=
                    (self.samples.len() / usize::from(self.info.channels)) as u64;
                return Ok(Some(&self.samples));
            }
        }
        self.check_finished().map(|()| None)
    }

    /// Seeks so the next decoded sample is at `position`. Returns the position reached.
    /// Seeking to or past the end of the track finishes it.
    pub fn seek(&mut self, position: Duration) -> Result<Duration, DecodeError> {
        if let Some(duration) = self.info.duration.filter(|&end| position >= end) {
            return Ok(self.finish_at(duration));
        }
        let target = self
            .position_to_timestamp(position)
            .max(self.playable.start);
        let decode_from = target.saturating_sub(self.seek_preroll);
        let from_start = decode_from <= self.first_ts;
        // Decoding a freshly opened file from its first packet is exact for every format (a
        // container seek near the start can skip a packet that begins before zero). Reopening
        // also recovers readers that cannot read again once they reached the end (MP4, CAF,
        // Matroska), and is where a scanning seek starts.
        if from_start || self.reader_at_end || self.seek_by_scanning {
            *self = Self::open(&self.path)?;
        }
        let audio_start = if from_start {
            Some(self.first_ts)
        } else if self.seek_by_scanning {
            self.scan_to(decode_from)?
        } else {
            self.seek_reader(decode_from)?
        };
        let Some(audio_start) = audio_start else {
            return Ok(self.finish_at(position));
        };
        // Audio before the target is decoded only to settle the decoder, then dropped.
        let required = target.max(audio_start);
        self.skip_until = Some(required);
        self.finished = false;
        Ok(self.timestamp_to_position(required))
    }

    /// Asks the format reader to seek so decoding starts at or before `ts`. Returns the
    /// timestamp the audio will start at, or `None` if `ts` is past the end of the track.
    fn seek_reader(&mut self, ts: i64) -> Result<Option<i64>, DecodeError> {
        let seek_to = SeekTo::Timestamp {
            ts: Timestamp::new(ts),
            track_id: self.track_id,
        };
        match self.format.seek(SeekMode::Accurate, seek_to) {
            Ok(seeked) => {
                self.decoder.reset();
                Ok(Some(seeked.required_ts.get().max(seeked.actual_ts.get())))
            }
            Err(error) if is_past_end(&error) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// Reads forward to the last packet that starts at or before `ts` (packet durations are
    /// often missing, so that is the one holding `ts`) and keeps it to be decoded next. Only
    /// the container is parsed, so this is fast. Returns where the kept audio starts, or
    /// `None` if the track has no packets.
    fn scan_to(&mut self, ts: i64) -> Result<Option<i64>, DecodeError> {
        let mut holding_ts: Option<Packet> = None;
        while let Some(packet) = self.next_packet()? {
            if packet.pts.get() > ts {
                let start = holding_ts.as_ref().unwrap_or(&packet).pts.get();
                self.pending.extend(holding_ts);
                self.pending.push_back(packet);
                return Ok(Some(start));
            }
            holding_ts = Some(packet);
        }
        // `ts` is in or after the last packet; decoding it finds out which.
        let start = holding_ts.as_ref().map(|packet| packet.pts.get());
        self.pending.extend(holding_ts);
        Ok(start)
    }

    /// Ends the track as if decoding had reached `position`.
    fn finish_at(&mut self, position: Duration) -> Duration {
        self.skip_until = None;
        self.finished = true;
        position
    }

    /// The next packet of the track, or `None` once the reader has reached the end.
    fn next_packet(&mut self) -> Result<Option<Packet>, DecodeError> {
        if let Some(packet) = self.pending.pop_front() {
            return Ok(Some(packet));
        }
        loop {
            match self.format.next_packet() {
                Ok(Some(packet)) if packet.track_id == self.track_id => return Ok(Some(packet)),
                Ok(Some(_)) => {}
                Ok(None) => break,
                // A truncated file simply ends early.
                Err(SymphoniaError::IoError(error))
                    if error.kind() == io::ErrorKind::UnexpectedEof =>
                {
                    break;
                }
                // A chained stream (such as concatenated Ogg files) starts a new track list,
                // which this decoder does not follow.
                Err(SymphoniaError::ResetRequired) => break,
                Err(error) => return Err(error.into()),
            }
        }
        self.reader_at_end = true;
        Ok(None)
    }

    fn note_bad_packet(&mut self) -> Result<(), DecodeError> {
        self.consecutive_errors += 1;
        self.skipped_packets += 1;
        if self.consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
            return Err(DecodeError::Malformed(
                "too many undecodable packets".into(),
            ));
        }
        Ok(())
    }

    /// A file that ends without producing any audio but had bad packets is undecodable.
    fn check_finished(&mut self) -> Result<(), DecodeError> {
        if self.frames_decoded == 0 && self.skipped_packets > 0 {
            return Err(DecodeError::Malformed("no decodable audio".into()));
        }
        if self.skipped_packets > 0 {
            warn!("skipped {} undecodable packets", self.skipped_packets);
            self.skipped_packets = 0;
        }
        Ok(())
    }

    /// Keeps only the decoded frames inside the playable range: drops those before
    /// `skip_until` and from the end of the track on. `first` is the timestamp of the first
    /// decoded frame.
    fn trim_to_playable(&mut self, first: i64) {
        let channels = usize::from(self.info.channels);
        let frames = self.samples.len() / channels;
        let keep_until = match self.playable.end {
            Some(end) if first >= end => {
                self.finished = true;
                0
            }
            Some(end) => self.timestamp_delta_to_frames(end - first).min(frames),
            None => frames,
        };
        let skip = match self.skip_until {
            Some(until) if first < until => self.timestamp_delta_to_frames(until - first),
            _ => 0,
        }
        .min(keep_until);
        if skip < keep_until {
            self.skip_until = None;
        }
        self.samples.truncate(keep_until * channels);
        self.samples.drain(..skip * channels);
    }

    fn timestamp_delta_to_frames(&self, delta: i64) -> usize {
        let delta = u128::try_from(delta).unwrap_or(0);
        let numer = u128::from(self.time_base.numer.get());
        let denom = u128::from(self.time_base.denom.get());
        let frames = delta * numer * u128::from(self.info.sample_rate) / denom;
        usize::try_from(frames).unwrap_or(usize::MAX)
    }

    fn position_to_timestamp(&self, position: Duration) -> i64 {
        duration_to_timestamp(self.time_base, position).saturating_add(self.playable.start)
    }

    fn timestamp_to_position(&self, ts: i64) -> Duration {
        timestamp_to_duration(self.time_base, ts.saturating_sub(self.playable.start))
            .unwrap_or_default()
    }
}

/// Symphonia's codecs plus the ones this crate adds.
fn codecs() -> &'static CodecRegistry {
    static REGISTRY: OnceLock<CodecRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut registry = CodecRegistry::new();
        symphonia::default::register_enabled_codecs(&mut registry);
        #[cfg(feature = "opus")]
        registry.register_audio_decoder::<crate::opus::OpusDecoder>();
        registry
    })
}

/// Where the audible audio of `track` lies.
///
/// Most demuxers give encoder delay negative timestamps, so position zero is timestamp zero
/// and the decoder trims the padding. Two cases need help: Ogg Opus timestamps include the
/// pre-skip (reported as the track's delay instead), and MP4 keeps delay and length in an
/// edit list that Symphonia does not apply.
fn playable_range(track: &Track, is_opus: bool, edit_list: Option<mp4::EditList>) -> Playable {
    let first = track.start_ts.get().max(0);
    if let Some(edit) = edit_list {
        let start = first.saturating_add(i64::try_from(edit.delay).unwrap_or(0));
        let end = edit
            .length
            .and_then(|length| i64::try_from(length).ok())
            .map(|length| start.saturating_add(length));
        return Playable { start, end };
    }
    let start = match track.delay {
        Some(delay) if is_opus && track.start_ts.get() >= 0 => first + i64::from(delay),
        _ => first,
    };
    Playable { start, end: None }
}

fn track_duration(
    track: &Track,
    sample_rate: u32,
    time_base: TimeBase,
    playable: Playable,
) -> Option<Duration> {
    if let Some(end) = playable.end {
        return timestamp_to_duration(time_base, end - playable.start);
    }
    // Both counts below run from the first timestamp, so they include any leading delay.
    let leading = u64::try_from(playable.start - track.start_ts.get().max(0)).unwrap_or(0);
    if let Some(frames) = track.num_frames {
        return Some(frames_to_duration(
            frames.saturating_sub(leading),
            sample_rate,
        ));
    }
    let length = track.duration?.get().saturating_sub(leading);
    time_base
        .calc_duration(units::Duration::new(length))
        .and_then(time_to_duration)
}

/// Frames to decode before a seek target so that decoders carrying state from packet to
/// packet produce the exact signal at the target: two frames for the overlapping transforms
/// of AAC and MPEG audio, two of the longest Vorbis blocks (Symphonia's Ogg reader adds
/// this itself, its Matroska reader does not), and 80 ms for Opus (RFC 7845 §4.6).
fn seek_preroll_frames(codec: AudioCodecId) -> u64 {
    match codec {
        CODEC_ID_OPUS => 3_840,
        CODEC_ID_AAC => 2 * 1_024,
        CODEC_ID_VORBIS => 2 * 4_096,
        CODEC_ID_MP1 | CODEC_ID_MP2 | CODEC_ID_MP3 => 2 * 1_152,
        _ => 0,
    }
}

/// The container's overall duration, for formats that give none per track (Matroska).
fn media_duration(info: &MediaInfo) -> Option<Duration> {
    info.time_base?
        .calc_duration(info.duration?)
        .and_then(time_to_duration)
}

/// Whether a seek failed because the target lies beyond the end of the stream.
fn is_past_end(error: &SymphoniaError) -> bool {
    match error {
        SymphoniaError::SeekError(SeekErrorKind::OutOfRange) => true,
        SymphoniaError::IoError(error) => error.kind() == io::ErrorKind::UnexpectedEof,
        _ => false,
    }
}

fn frames_to_duration(frames: u64, sample_rate: u32) -> Duration {
    let nanos = u128::from(frames) * 1_000_000_000 / u128::from(sample_rate.max(1));
    Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
}

fn duration_to_timestamp(time_base: TimeBase, duration: Duration) -> i64 {
    let seconds = i64::try_from(duration.as_secs()).unwrap_or(i64::MAX);
    Time::try_new(seconds, duration.subsec_nanos())
        .and_then(|time| time_base.calc_timestamp(time))
        .map_or(i64::MAX, Timestamp::get)
}

fn timestamp_to_duration(time_base: TimeBase, ts: i64) -> Option<Duration> {
    time_base
        .calc_time(Timestamp::new(ts))
        .and_then(time_to_duration)
}

fn time_to_duration(time: Time) -> Option<Duration> {
    let (seconds, nanos) = time.parts();
    u64::try_from(seconds)
        .ok()
        .map(|seconds| Duration::new(seconds, nanos))
}

/// Copies a decoded buffer into `out` as interleaved samples with exactly `channels` channels.
fn copy_interleaved(buffer: &GenericAudioBufferRef<'_>, channels: usize, out: &mut Vec<f32>) {
    let decoded_channels = buffer.spec().channels().count();
    buffer.copy_to_vec_interleaved(out);
    if decoded_channels == channels || decoded_channels == 0 {
        return;
    }
    // The stream changed its channel count mid-way (rare). Map it onto the layout the output
    // was configured for rather than reconfiguring the device.
    let decoded = std::mem::take(out);
    out.reserve(decoded.len() / decoded_channels * channels);
    for frame in decoded.chunks_exact(decoded_channels) {
        out.extend((0..channels).map(|channel| frame[channel % decoded_channels]));
    }
}
