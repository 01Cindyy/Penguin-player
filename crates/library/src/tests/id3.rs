//! Hand-built ID3v2 tags that lofty must not be allowed to choke on.

use std::fs;

use super::fixtures::TestLibrary;
use crate::chapter_limit::MAX_CHAPTER_FRAMES;

fn synchsafe(value: usize) -> [u8; 4] {
    let value = u32::try_from(value).expect("test tags are small");
    [21, 14, 7, 0].map(|shift| ((value >> shift) & 0x7F) as u8)
}

/// An ID3v2.4 frame with the given format flags (the second flags byte).
fn frame(id: &[u8; 4], format_flags: u8, content: &[u8]) -> Vec<u8> {
    [
        id.as_slice(),
        &synchsafe(content.len()),
        &[0, format_flags],
        content,
    ]
    .concat()
}

fn text_frame(id: &[u8; 4], text: &str) -> Vec<u8> {
    let content = [&[3], text.as_bytes()].concat();
    frame(id, 0, &content)
}

/// A `CHAP` frame (element id, start/end times and offsets) holding `children`.
fn chapter(children: &[u8]) -> Vec<u8> {
    frame(
        b"CHAP",
        0,
        &[b"c\0".as_slice(), &[0; 16], children].concat(),
    )
}

/// An MP3 file: an ID3v2.4 tag holding `frames`, then a few silent MPEG-1 Layer III
/// frames (128 kbps at 44.1 kHz, 417 bytes each).
fn mp3_with_tag(frames: &[u8]) -> Vec<u8> {
    let mut file = [b"ID3\x04\0\0".as_slice(), &synchsafe(frames.len()), frames].concat();
    for _ in 0..20 {
        let mut audio_frame = vec![0xFF, 0xFB, 0x90, 0x00];
        audio_frame.resize(417, 0);
        file.extend_from_slice(&audio_frame);
    }
    file
}

/// A zlib stream of `data` in one stored (uncompressed) block.
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let len = u16::try_from(data.len()).expect("test data is small");
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + u32::from(byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    [
        &[0x78, 0x01, 0x01],
        len.to_le_bytes().as_slice(),
        &(!len).to_le_bytes(),
        data,
        &((b << 16) | a).to_be_bytes(),
    ]
    .concat()
}

/// lofty parses chapters inside chapters recursively, and a few kilobytes of them used
/// to overflow the stack, aborting the whole app on every scan until the file was gone.
#[test]
fn deeply_nested_chapters_do_not_abort_the_scan() {
    let mut lib = TestLibrary::new();
    let nested = (0..4 * MAX_CHAPTER_FRAMES).fold(Vec::new(), |inner, _| chapter(&inner));
    let tag = [text_frame(b"TIT2", "Nested"), nested].concat();
    let path = lib.path("Crafted/nested.mp3");
    fs::write(&path, mp3_with_tag(&tag)).unwrap();

    assert_eq!(lib.scan().added, 1);
    // The tags are skipped, but the audio is still there to play.
    let track = lib.track_at(&path).unwrap();
    assert_eq!(track.title, "nested");
    assert!(track.duration_ms > 0);
}

#[test]
fn files_with_chapters_keep_their_tags() {
    let mut lib = TestLibrary::new();
    let chapters: Vec<u8> = (0..20)
        .flat_map(|_| chapter(&text_frame(b"TIT2", "Chapter")))
        .collect();
    let tag = [
        text_frame(b"TIT2", "Episode 1"),
        text_frame(b"TPE1", "Host"),
        frame(b"CTOC", 0, b"toc\0\x03\x01c\0"),
        chapters,
    ]
    .concat();
    let path = lib.path("Podcast/episode.mp3");
    fs::write(&path, mp3_with_tag(&tag)).unwrap();

    assert_eq!(lib.scan().added, 1);
    let track = lib.track_at(&path).unwrap();
    assert_eq!(
        (track.title.as_str(), track.artist.as_str()),
        ("Episode 1", "Host")
    );
}

/// Compressed frames are rare but valid ID3v2 (see the known limit in `chapter_limit`).
#[test]
fn compressed_frames_are_read() {
    let mut lib = TestLibrary::new();
    let title = [&[3], "Hidden".as_bytes()].concat();
    let content = [synchsafe(title.len()).as_slice(), &zlib_stored(&title)].concat();
    // Format flags: compressed (0x08), with a data length indicator (0x01).
    let tag = frame(b"TIT2", 0x09, &content);
    let path = lib.path("compressed.mp3");
    fs::write(&path, mp3_with_tag(&tag)).unwrap();

    assert_eq!(lib.scan().added, 1);
    assert_eq!(lib.track_at(&path).unwrap().title, "Hidden");
}
