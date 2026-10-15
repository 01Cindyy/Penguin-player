//! Tests of `TrackDecoder` on generated WAV files.

mod common;

use std::path::PathBuf;
use std::time::Duration;

use mp_audio::{DecodeError, TrackDecoder};
use tempfile::TempDir;

const RATE: u32 = 8_000;
const FRAMES: u32 = 16_000;

/// A stereo WAV file in which every frame holds its own index, so a decoded sample tells
/// exactly which frame it came from.
fn indexed_wav(dir: &TempDir) -> PathBuf {
    let path = dir.path().join("indexed.wav");
    common::write_wav(&path, RATE, 2, FRAMES, |frame| frame as i16);
    path
}

fn frame_index(sample: f32) -> u32 {
    (sample * 32_768.0).round() as u32
}

fn frame_position(frame: u32) -> Duration {
    Duration::from_nanos(u64::from(frame) * 1_000_000_000 / u64::from(RATE))
}

fn decode_rest(decoder: &mut TrackDecoder) -> Vec<f32> {
    let mut samples = Vec::new();
    while let Some(chunk) = decoder.next_chunk().expect("decodes") {
        samples.extend_from_slice(chunk);
    }
    samples
}

#[test]
fn reports_stream_info_and_decodes_every_frame() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut decoder = TrackDecoder::open(&indexed_wav(&dir)).expect("opens");
    let info = decoder.info().clone();
    assert_eq!((info.sample_rate, info.channels), (RATE, 2));
    assert_eq!(info.duration, Some(Duration::from_secs(2)));

    let samples = decode_rest(&mut decoder);
    assert_eq!(samples.len(), 2 * FRAMES as usize);
    assert_eq!(frame_index(samples[2 * 12_345]), 12_345);
}

#[test]
fn seeks_to_the_exact_frame() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut decoder = TrackDecoder::open(&indexed_wav(&dir)).expect("opens");
    for frame in [12_345, 1, 4_000, 0, FRAMES - 1] {
        let position = frame_position(frame);
        assert_eq!(decoder.seek(position).expect("seeks"), position);
        let chunk = decoder.next_chunk().expect("decodes").expect("audio left");
        assert_eq!(
            frame_index(chunk[0]),
            frame,
            "after seeking to {position:?}"
        );
    }
}

#[test]
fn seeking_past_the_end_finishes_the_track() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut decoder = TrackDecoder::open(&indexed_wav(&dir)).expect("opens");
    assert_eq!(
        decoder.seek(Duration::from_secs(30)).expect("seeks"),
        Duration::from_secs(2)
    );
    assert!(decoder.next_chunk().expect("decodes").is_none());

    // Seeking back in resumes decoding.
    decoder.seek(frame_position(FRAMES - 100)).expect("seeks");
    assert_eq!(decode_rest(&mut decoder).len(), 2 * 100);
}

#[test]
fn seeks_after_reading_an_m4a_to_the_end() {
    // Symphonia's MP4 reader cannot read again once it has reached the end of the file.
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("indexed.m4a");
    common::write_m4a(&path, RATE, FRAMES, |frame| frame as i16);
    let mut decoder = TrackDecoder::open(&path).expect("opens");
    assert_eq!(decode_rest(&mut decoder).len(), 2 * FRAMES as usize);

    for frame in [4_000, 0] {
        let position = frame_position(frame);
        assert_eq!(decoder.seek(position).expect("seeks"), position);
        let rest = decode_rest(&mut decoder);
        assert_eq!(rest.len(), 2 * (FRAMES - frame) as usize);
        assert_eq!(frame_index(rest[0]), frame, "after seeking to {position:?}");
    }
}

/// Seeks back and forth in a Matroska file (whose timestamps are whole milliseconds), past
/// the first cluster and back, and checks each lands on the exact frame.
fn check_matroska_seeks(cues: bool) {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("indexed.mka");
    common::write_mka(&path, RATE, FRAMES, cues, |frame| frame as i16);
    let mut decoder = TrackDecoder::open(&path).expect("opens");
    assert_eq!(decoder.info().duration, Some(frame_position(FRAMES)));

    for frame in [12_000, 4_000, 15_200, 8, 9_600] {
        let position = frame_position(frame);
        assert_eq!(decoder.seek(position).expect("seeks"), position);
        let rest = decode_rest(&mut decoder);
        assert_eq!(rest.len(), 2 * (FRAMES - frame) as usize);
        assert_eq!(frame_index(rest[0]), frame, "after seeking to {position:?}");
    }
}

#[test]
fn seeks_in_matroska_with_cues() {
    check_matroska_seeks(true);
}

#[test]
fn seeks_in_matroska_without_cues() {
    check_matroska_seeks(false);
}

#[test]
fn rejects_corrupt_sample_rates() {
    let dir = tempfile::tempdir().expect("temp dir");
    for rate in [1, 2_000_000_000] {
        let path = dir.path().join(format!("{rate}.wav"));
        common::write_wav(&path, rate, 1, 200, |frame| frame as i16);
        assert!(
            matches!(TrackDecoder::open(&path), Err(DecodeError::Unsupported(_))),
            "{rate} Hz was accepted"
        );
    }
}

#[test]
fn rejects_files_that_are_not_audio() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("notes.flac");
    std::fs::write(&path, b"definitely not a flac file").expect("write");
    assert!(matches!(
        TrackDecoder::open(&path),
        Err(DecodeError::Unsupported(_) | DecodeError::Malformed(_) | DecodeError::Io(_))
    ));
}
