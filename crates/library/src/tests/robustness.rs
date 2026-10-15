//! Broken, truncated and hostile files: the scan must finish, keep the good files and
//! never panic.

use std::fs;

use lofty::picture::PictureType;
use mp_core::format::SUPPORTED_EXTENSIONS;
use mp_core::util::stable_hash;
use mp_core::{AudioFormat, CoverId};

use super::fixtures::{TestLibrary, TestTags, png, wav_bytes, write_wav};
use crate::ScanSummary;

/// An MP4 box: big-endian size, four-character type, payload.
fn mp4_box(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let size = u32::try_from(8 + payload.len()).expect("test boxes are small");
    [&size.to_be_bytes(), kind.as_slice(), payload].concat()
}

/// The smallest MP4 that lofty reads as audio: one sound track whose `mdhd` box declares
/// `duration` units at `timescale` units per second.
fn mp4_with_duration(timescale: u32, duration: u64) -> Vec<u8> {
    let mut mdhd = vec![1, 0, 0, 0];
    mdhd.extend_from_slice(&[0; 16]);
    mdhd.extend_from_slice(&timescale.to_be_bytes());
    mdhd.extend_from_slice(&duration.to_be_bytes());
    mdhd.extend_from_slice(&[0; 4]);
    let mut hdlr = vec![0; 8];
    hdlr.extend_from_slice(b"soun");
    hdlr.extend_from_slice(&[0; 13]);
    let mdia = [mp4_box(b"mdhd", &mdhd), mp4_box(b"hdlr", &hdlr)].concat();
    let moov = mp4_box(b"trak", &mp4_box(b"mdia", &mdia));
    [
        mp4_box(b"ftyp", b"M4A \0\0\0\0M4A isom"),
        mp4_box(b"moov", &moov),
    ]
    .concat()
}

/// A duration too large for SQLite's signed integers doesn't fail the whole scan's
/// write, so one crafted file can't keep every other file out of the library.
#[test]
fn absurd_durations_do_not_break_the_scan() {
    let mut lib = TestLibrary::new();
    let crafted = lib.path("crafted.m4a");
    fs::write(&crafted, mp4_with_duration(1, 10_000_000_000_000_000)).unwrap();
    write_wav(&lib.path("fine.wav"), &TestTags::default());

    assert_eq!(lib.scan().added, 2);
    let track = lib.track_at(&crafted).unwrap();
    assert_eq!(track.format, AudioFormat::Aac);
    assert_eq!(track.duration_ms, 0);
}

/// lofty refuses tag items over 16 MiB by default; a file with such a picture is still
/// read, cover included, rather than dropped from the library.
#[test]
fn files_with_huge_embedded_pictures_are_read_with_their_cover() {
    let mut lib = TestLibrary::new();
    let mut huge = png(1);
    huge.resize(17 * 1024 * 1024, 0);
    let path = lib.path("Album/01.wav");
    write_wav(
        &path,
        &TestTags {
            title: "Huge Art",
            artist: "Big Scans",
            pictures: vec![(PictureType::CoverFront, huge.clone())],
            ..TestTags::default()
        },
    );

    assert_eq!(lib.scan().added, 1);
    let track = lib.track_at(&path).unwrap();
    assert_eq!(
        (track.title.as_str(), track.artist.as_str()),
        ("Huge Art", "Big Scans")
    );
    let expected = CoverId(format!("{:016x}.png", stable_hash(&huge)));
    assert_eq!(track.cover, Some(expected.clone()));
    let stored = fs::metadata(lib.covers.join(&expected.0)).unwrap();
    assert_eq!(stored.len(), huge.len() as u64);
}

#[test]
fn empty_garbage_and_truncated_files_never_stop_a_scan() {
    let mut lib = TestLibrary::new();
    let wav = wav_bytes(44_100, 2, 16, 4_410);
    let mut broken = 0;
    for extension in SUPPORTED_EXTENSIONS {
        fs::write(lib.path(&format!("empty.{extension}")), b"").unwrap();
        fs::write(lib.path(&format!("text.{extension}")), b"not audio").unwrap();
        // A real WAV header cut short, under every extension.
        fs::write(lib.path(&format!("cut.{extension}")), &wav[..40]).unwrap();
        broken += 3;
    }
    write_wav(&lib.path("fine.wav"), &TestTags::default());

    let summary = lib.scan();
    assert_eq!(summary.added + summary.failed, broken + 1);
    assert!(lib.library.track_count().unwrap() >= 1);
    // Failed files are retried, but nothing that was added is re-read.
    let rescan = lib.scan();
    assert_eq!(
        rescan,
        ScanSummary {
            unchanged: summary.added,
            failed: summary.failed,
            ..ScanSummary::default()
        }
    );
}
