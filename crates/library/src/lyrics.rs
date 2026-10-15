//! Finding a song's lyrics: in an `.lrc` file beside it, or in its tags. Lyrics that
//! plugins find online are kept in a cache, or beside the song if the user wants.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::thread;

use lofty::config::ParseOptions;
use lofty::tag::ItemKey;
use mp_core::Lyrics;
use mp_core::util::stable_hash;

use crate::chapter_limit::TAG_READER_STACK_BYTES;
use crate::tags::{self, TagMode};

/// Lyrics files larger than this aren't read.
const MAX_LRC_BYTES: u64 = 1024 * 1024;

/// The lyrics of the song at `path`, from an `.lrc` file with the same name beside it
/// or from its tags. Timed lyrics win over plain ones, then the `.lrc` file over the
/// tags. Reads files, so it belongs off the UI thread.
pub fn read_lyrics(path: &Path) -> Option<Lyrics> {
    let beside = lrc_file(path)
        .and_then(|file| read_text(&file))
        .and_then(|text| Lyrics::parse(&text));
    if beside.as_ref().is_some_and(Lyrics::is_synced) {
        return beside;
    }
    let tagged = tagged_lyrics(path);
    if tagged.as_ref().is_some_and(Lyrics::is_synced) {
        tagged
    } else {
        beside.or(tagged)
    }
}

/// Lyrics a plugin found for the song at `path`, if [`save_found_lyrics`] kept them in
/// `cache_dir`.
pub fn read_found_lyrics(cache_dir: &Path, song: &Path) -> Option<Lyrics> {
    read_text(&cached_file(cache_dir, song)).and_then(|text| Lyrics::parse(&text))
}

/// Keeps lyrics a plugin found for the song at `song`, so they are looked up only once:
/// as an `.lrc` file beside the song if `beside` (unless one is there already, or the
/// folder can't be written to), otherwise in `cache_dir`. Returns where they went.
pub fn save_found_lyrics(
    cache_dir: &Path,
    song: &Path,
    text: &str,
    beside: bool,
) -> io::Result<PathBuf> {
    if beside {
        let lrc = song.with_extension("lrc");
        let written = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lrc)
            .and_then(|mut file| file.write_all(text.as_bytes()));
        match written {
            Ok(()) => return Ok(lrc),
            Err(err) => log::info!("Keeping lyrics in the cache, not {}: {err}", lrc.display()),
        }
    }
    fs::create_dir_all(cache_dir)?;
    let file = cached_file(cache_dir, song);
    let temporary = file.with_extension("lrc.tmp");
    fs::write(&temporary, text)?;
    fs::rename(&temporary, &file)?;
    Ok(file)
}

fn cached_file(cache_dir: &Path, song: &Path) -> PathBuf {
    let key = stable_hash(song.as_os_str().as_encoded_bytes());
    cache_dir.join(format!("{key:016x}.lrc"))
}

fn lrc_file(song: &Path) -> Option<PathBuf> {
    ["lrc", "LRC"]
        .into_iter()
        .map(|extension| song.with_extension(extension))
        .find(|file| file.is_file())
}

fn read_text(file: &Path) -> Option<String> {
    if fs::metadata(file).ok()?.len() > MAX_LRC_BYTES {
        return None;
    }
    fs::read(file).ok().map(|bytes| decode(&bytes))
}

/// UTF-8, or UTF-16 with a byte order mark. Anything else is taken as Latin-1, which
/// older lyrics files often are.
fn decode(bytes: &[u8]) -> String {
    let utf16 = |from_bytes: fn([u8; 2]) -> u16| {
        let units: Vec<u16> = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&pair| from_bytes(pair))
            .collect();
        String::from_utf16_lossy(&units)
    };
    match bytes {
        [0xFF, 0xFE, ..] => utf16(u16::from_le_bytes),
        [0xFE, 0xFF, ..] => utf16(u16::from_be_bytes),
        _ => match std::str::from_utf8(bytes) {
            Ok(text) => text.to_owned(),
            Err(_) => bytes.iter().map(|&byte| char::from(byte)).collect(),
        },
    }
}

/// Lyrics from the tags (`LYRICS`, `UNSYNCEDLYRICS`, ID3v2 `USLT`, MP4 `©lyr`), read
/// on a thread with the stack lofty may need (see `chapter_limit`).
fn tagged_lyrics(path: &Path) -> Option<Lyrics> {
    let path = path.to_path_buf();
    thread::Builder::new()
        .name("lyrics".into())
        .stack_size(TAG_READER_STACK_BYTES)
        .spawn(move || read_tagged(&path))
        .ok()?
        .join()
        .ok()
        .flatten()
}

fn read_tagged(path: &Path) -> Option<Lyrics> {
    tags::allow_tag_items_up_to(fs::metadata(path).ok()?.len());
    let options = ParseOptions::new()
        .read_cover_art(false)
        .read_properties(false);
    let file = tags::open(path, TagMode::Read)
        .ok()?
        .options(options)
        .guess_file_type()
        .ok()?
        .read()
        .ok()?;
    let tags = tags::tags_by_priority(&file);
    let mut found = [ItemKey::Lyrics, ItemKey::UnsyncLyrics]
        .into_iter()
        .flat_map(|key| tags::values(&tags, key))
        .filter_map(Lyrics::parse);
    let first = found.next()?;
    if first.is_synced() {
        return Some(first);
    }
    Some(found.find(Lyrics::is_synced).unwrap_or(first))
}
