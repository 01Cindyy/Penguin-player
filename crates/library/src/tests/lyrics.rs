use std::fs;
use std::path::PathBuf;

use mp_core::Lyrics;

use super::fixtures::{TestLibrary, TestTags, write_wav};
use crate::{read_found_lyrics, read_lyrics, save_found_lyrics};

const TIMED: &str = "[ar:Someone]\r\n[00:01.50] Timed one\r\n[00:03.00] Timed two\r\n";
const PLAIN: &str = "Plain one\r\nPlain two\r\n";

fn song(lib: &TestLibrary, lyrics_tag: &'static str) -> PathBuf {
    let path = lib.path("Artist - Song.wav");
    let tags = TestTags {
        title: "Song",
        lyrics: lyrics_tag,
        ..TestTags::default()
    };
    write_wav(&path, &tags);
    path
}

fn texts(lyrics: &Lyrics) -> Vec<&str> {
    lyrics
        .lines()
        .iter()
        .map(|line| line.text.as_str())
        .collect()
}

#[test]
fn lrc_file_beside_the_song_is_read() {
    let lib = TestLibrary::new();
    let song = song(&lib, "");
    fs::write(lib.path("Artist - Song.lrc"), TIMED).unwrap();

    let lyrics = read_lyrics(&song).unwrap();
    assert!(lyrics.is_synced());
    assert_eq!(texts(&lyrics), ["Timed one", "Timed two"]);
}

#[test]
fn upper_case_extension_is_found() {
    let lib = TestLibrary::new();
    let song = song(&lib, "");
    fs::write(lib.path("Artist - Song.LRC"), PLAIN).unwrap();
    assert_eq!(
        texts(&read_lyrics(&song).unwrap()),
        ["Plain one", "Plain two"]
    );
}

#[test]
fn lyrics_in_the_tags_are_read() {
    let lib = TestLibrary::new();
    let song = song(&lib, "Tagged one\nTagged two");

    let lyrics = read_lyrics(&song).unwrap();
    assert!(!lyrics.is_synced());
    assert_eq!(texts(&lyrics), ["Tagged one", "Tagged two"]);
}

#[test]
fn timed_lyrics_win_then_the_lrc_file() {
    let lib = TestLibrary::new();
    let lrc = lib.path("Artist - Song.lrc");

    let song = song(&lib, TIMED);
    fs::write(&lrc, PLAIN).unwrap();
    assert_eq!(
        texts(&read_lyrics(&song).unwrap()),
        ["Timed one", "Timed two"]
    );

    let song = self::song(&lib, "Tagged");
    fs::write(&lrc, TIMED).unwrap();
    assert_eq!(
        texts(&read_lyrics(&song).unwrap()),
        ["Timed one", "Timed two"]
    );

    fs::write(&lrc, PLAIN).unwrap();
    assert_eq!(
        texts(&read_lyrics(&song).unwrap()),
        ["Plain one", "Plain two"]
    );
}

#[test]
fn songs_without_lyrics_have_none() {
    let lib = TestLibrary::new();
    let song = song(&lib, "");
    assert_eq!(read_lyrics(&song), None);
    // Empty lyrics files and broken songs are no lyrics, not errors.
    fs::write(lib.path("Artist - Song.lrc"), "\r\n[ti:Song]\r\n").unwrap();
    assert_eq!(read_lyrics(&song), None);
    let broken = lib.path("broken.flac");
    fs::write(&broken, "not a flac file").unwrap();
    assert_eq!(read_lyrics(&broken), None);
    assert_eq!(read_lyrics(&lib.path("missing.mp3")), None);
}

#[test]
fn older_text_encodings_are_read() {
    let lib = TestLibrary::new();
    let song = song(&lib, "");
    let lrc = lib.path("Artist - Song.lrc");

    let utf16: Vec<u8> = [0xFF, 0xFE]
        .into_iter()
        .chain("[00:01.00]Café".encode_utf16().flat_map(u16::to_le_bytes))
        .collect();
    fs::write(&lrc, utf16).unwrap();
    assert_eq!(texts(&read_lyrics(&song).unwrap()), ["Café"]);

    fs::write(&lrc, b"Caf\xe9 au lait").unwrap();
    assert_eq!(texts(&read_lyrics(&song).unwrap()), ["Café au lait"]);
}

#[test]
fn found_lyrics_are_kept_in_the_cache() {
    let lib = TestLibrary::new();
    let song = song(&lib, "");
    let cache = lib.path("cache");
    assert_eq!(read_found_lyrics(&cache, &song), None);

    let saved = save_found_lyrics(&cache, &song, TIMED, false).unwrap();
    assert!(saved.starts_with(&cache));
    assert_eq!(
        texts(&read_found_lyrics(&cache, &song).unwrap()),
        ["Timed one", "Timed two"]
    );
    // Only for this song, and never in place of its own lyrics.
    assert_eq!(read_found_lyrics(&cache, &lib.path("other.wav")), None);
    assert_eq!(read_lyrics(&song), None);
}

#[test]
fn found_lyrics_can_go_beside_the_song_but_never_replace_a_file() {
    let lib = TestLibrary::new();
    let song = song(&lib, "");
    let cache = lib.path("cache");
    let lrc = lib.path("Artist - Song.lrc");

    assert_eq!(save_found_lyrics(&cache, &song, PLAIN, true).unwrap(), lrc);
    assert_eq!(
        texts(&read_lyrics(&song).unwrap()),
        ["Plain one", "Plain two"]
    );

    let saved = save_found_lyrics(&cache, &song, TIMED, true).unwrap();
    assert!(saved.starts_with(&cache));
    assert_eq!(fs::read_to_string(&lrc).unwrap(), PLAIN);
}
