//! Reading tags and stream properties from audio files with lofty.

use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::Path;
use std::time::Duration;

use lofty::config::{GlobalOptions, ParseOptions, apply_global_options};
use lofty::error::FileParseError;
use lofty::file::{AudioFile, FileType, TaggedFile, TaggedFileExt};
use lofty::mp4::{Mp4Codec, Mp4File};
use lofty::picture::{Picture, PictureType};
use lofty::probe::Probe;
use lofty::properties::FileProperties;
use lofty::tag::{ItemKey, Tag};
use mp_core::{AudioFormat, Track, TrackId};

use crate::chapter_limit::ChapterLimit;

/// Tags that may hold the release date, most specific first.
const DATE_KEYS: [ItemKey; 4] = [
    ItemKey::RecordingDate,
    ItemKey::Year,
    ItemKey::ReleaseDate,
    ItemKey::OriginalReleaseDate,
];

/// The first bytes of containers lofty can't read but the player can play: Matroska
/// and WebM, CAF, and Ogg holding a codec lofty doesn't know (such as FLAC).
const UNPARSED_CONTAINER_MAGIC: [&[u8; 4]; 3] = [b"\x1A\x45\xDF\xA3", b"caff", b"OggS"];

/// What reading one audio file produced.
pub(crate) struct FileInfo {
    /// The track, with a placeholder id, `added` = 0 and no cover: the scanner fills those in.
    pub track: Track,
    /// The embedded picture to use as cover art, if any.
    pub picture: Option<Picture>,
}

/// The most lofty may allocate for one tag item, such as an embedded picture or a WAV
/// file's whole ID3 chunk.
const MAX_TAG_ITEM_BYTES: usize = 256 * 1024 * 1024;

/// Reads an audio file's tags and stream properties. Must run on a thread with a stack
/// of at least [`crate::chapter_limit::TAG_READER_STACK_BYTES`].
///
/// Files in containers lofty doesn't know (see [`UNPARSED_CONTAINER_MAGIC`]) still give
/// a track, titled after the file name with the format guessed from the extension. So
/// do files whose tags can't be parsed. Anything else lofty can't parse is an error.
pub(crate) fn read(path: &Path, file_size: u64, modified: i64) -> Result<FileInfo, FileParseError> {
    allow_tag_items_up_to(file_size);
    let read = |options, mode| read_with(path, file_size, modified, options, mode);
    read(ParseOptions::new(), TagMode::Read).or_else(|error| {
        // A broken or oversized picture fails the whole file; the rest may still be fine.
        read(ParseOptions::new().read_cover_art(false), TagMode::Read).or_else(|_| {
            let untagged =
                read(ParseOptions::new().read_tags(false), TagMode::Skip).map_err(|_| error)?;
            log::warn!(
                "Ignoring the tags of {}, which can't be read",
                path.display()
            );
            Ok(untagged)
        })
    })
}

/// lofty refuses to allocate more than 16 MiB for a tag item by default, so a file with
/// a bigger embedded picture can't be read at all. No item can be larger than the file
/// holding it, so on this thread allow up to the file's size (within a sane maximum).
/// Small crafted files claiming huge items still can't make lofty allocate much.
pub(crate) fn allow_tag_items_up_to(file_size: u64) {
    let limit = usize::try_from(file_size)
        .unwrap_or(usize::MAX)
        .clamp(GlobalOptions::DEFAULT_ALLOCATION_LIMIT, MAX_TAG_ITEM_BYTES);
    apply_global_options(GlobalOptions::new().allocation_limit(limit));
}

fn read_with(
    path: &Path,
    file_size: u64,
    modified: i64,
    options: ParseOptions,
    mode: TagMode,
) -> Result<FileInfo, FileParseError> {
    let mut track = untagged_track(path, file_size, modified);
    let probe = open(path, mode)?.options(options).guess_file_type()?;
    let Some(file_type) = probe.file_type() else {
        if !is_unparsed_container(probe.into_inner()) {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "not an audio file").into());
        }
        return Ok(FileInfo {
            track,
            picture: None,
        });
    };

    let file = match file_type {
        // Only the MP4 reader knows the codec inside, which tells ALAC from AAC.
        FileType::Mp4 => {
            let mp4 = Mp4File::read_from(&mut probe.into_inner(), options)?;
            track.format = mp4_format(mp4.properties().codec());
            TaggedFile::from(mp4)
        }
        other => {
            track.format = file_type_format(other);
            probe.read()?
        }
    };

    apply_properties(&mut track, file.properties());
    let tags = tags_by_priority(&file);
    apply_tags(&mut track, &tags);
    Ok(FileInfo {
        track,
        picture: cover_picture(&tags).cloned(),
    })
}

/// Whether lofty parses a file's tags or seeks past them (which `options` must match).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TagMode {
    Read,
    Skip,
}

/// Like `Probe::open`, but lofty reads the file through a [`ChapterLimit`] when it
/// will parse the tags.
pub(crate) fn open(path: &Path, mode: TagMode) -> io::Result<Probe<BufReader<ChapterLimit<File>>>> {
    let file = File::open(path)?;
    let limited = if mode == TagMode::Read {
        ChapterLimit::new(file)
    } else {
        ChapterLimit::unlimited(file)
    };
    let reader = BufReader::new(limited);
    Ok(match FileType::from_path(path) {
        Some(guess) => Probe::with_file_type(reader, guess),
        None => Probe::new(reader),
    })
}

/// Whether the stream starts like one of [`UNPARSED_CONTAINER_MAGIC`].
fn is_unparsed_container(mut stream: impl Read) -> bool {
    let mut magic = [0; 4];
    stream.read_exact(&mut magic).is_ok() && UNPARSED_CONTAINER_MAGIC.contains(&&magic)
}

/// A track with only what the path tells: the file name as title and the format
/// guessed from the extension.
fn untagged_track(path: &Path, file_size: u64, modified: i64) -> Track {
    let title = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().trim().to_owned())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| path.display().to_string());
    let format = path
        .extension()
        .and_then(|ext| ext.to_str())
        .and_then(AudioFormat::from_extension)
        .unwrap_or(AudioFormat::Unknown);
    Track {
        id: TrackId(0),
        path: path.to_path_buf(),
        title,
        artist: String::new(),
        album: String::new(),
        album_artist: String::new(),
        genre: String::new(),
        year: None,
        track_number: None,
        disc_number: None,
        duration_ms: 0,
        format,
        sample_rate: None,
        bit_depth: None,
        channels: None,
        bitrate_kbps: None,
        file_size,
        modified,
        added: 0,
        cover: None,
    }
}

fn mp4_format(codec: Option<Mp4Codec>) -> AudioFormat {
    match codec {
        Some(Mp4Codec::ALAC) => AudioFormat::Alac,
        Some(Mp4Codec::FLAC) => AudioFormat::Flac,
        Some(Mp4Codec::MP3) => AudioFormat::Mp3,
        _ => AudioFormat::Aac,
    }
}

fn file_type_format(file_type: FileType) -> AudioFormat {
    match file_type {
        FileType::Flac => AudioFormat::Flac,
        FileType::Mpeg => AudioFormat::Mp3,
        FileType::Aac | FileType::Mp4 => AudioFormat::Aac,
        FileType::Wav => AudioFormat::Wav,
        FileType::Aiff => AudioFormat::Aiff,
        FileType::Opus => AudioFormat::Opus,
        FileType::Vorbis => AudioFormat::Vorbis,
        _ => AudioFormat::Unknown,
    }
}

fn apply_properties(track: &mut Track, properties: &FileProperties) {
    track.duration_ms = storable_millis(properties.duration());
    track.sample_rate = properties.sample_rate().filter(|&rate| rate > 0);
    track.bit_depth = properties.bit_depth().filter(|&bits| bits > 0);
    track.channels = properties.channels().filter(|&channels| channels > 0);
    track.bitrate_kbps = properties
        .audio_bitrate()
        .or(properties.overall_bitrate())
        .filter(|&kbps| kbps > 0);
}

/// The duration in milliseconds, or 0 ("unknown") if it doesn't fit the database's
/// signed 64-bit integers. Only a corrupt or crafted header claims that much, and an
/// unstorable value would make the scan's whole write fail.
fn storable_millis(duration: Duration) -> u64 {
    i64::try_from(duration.as_millis())
        .ok()
        .and_then(|millis| u64::try_from(millis).ok())
        .unwrap_or(0)
}

/// The file's tags with its primary tag first, so e.g. ID3v2 wins over ID3v1.
pub(crate) fn tags_by_priority(file: &TaggedFile) -> Vec<&Tag> {
    let primary = file.primary_tag_type();
    let mut tags: Vec<&Tag> = file.tags().iter().collect();
    tags.sort_by_key(|tag| tag.tag_type() != primary);
    tags
}

fn apply_tags(track: &mut Track, tags: &[&Tag]) {
    if let Some(title) = values(tags, ItemKey::TrackTitle).next() {
        track.title = title.to_owned();
    }
    track.artist = text(tags, ItemKey::TrackArtist);
    track.album = text(tags, ItemKey::AlbumTitle);
    track.album_artist = text(tags, ItemKey::AlbumArtist);
    track.genre = text(tags, ItemKey::Genre);
    track.year = DATE_KEYS
        .into_iter()
        .flat_map(|key| values(tags, key))
        .find_map(parse_year);
    track.track_number = values(tags, ItemKey::TrackNumber).find_map(parse_number);
    track.disc_number = values(tags, ItemKey::DiscNumber).find_map(parse_number);
}

/// Non-empty values of `key`, trimmed, from every tag in priority order.
pub(crate) fn values<'a>(tags: &'a [&'a Tag], key: ItemKey) -> impl Iterator<Item = &'a str> {
    tags.iter()
        .flat_map(move |tag| tag.get_strings(key))
        .map(|value| value.trim_matches(|c: char| c.is_whitespace() || c == '\0'))
        .filter(|value| !value.is_empty())
}

fn text(tags: &[&Tag], key: ItemKey) -> String {
    values(tags, key).next().unwrap_or_default().to_owned()
}

/// The year in a date such as "2019", "2019-05-01", "05/01/2019" or "20190501": the
/// first number with four digits, or eight (a date written without separators).
fn parse_year(date: &str) -> Option<u32> {
    date.split(|c: char| !c.is_ascii_digit())
        .find(|digits| digits.len() == 4 || digits.len() == 8)
        .and_then(|digits| digits[..4].parse().ok())
        .filter(|&year| year > 0)
}

/// A track or disc number such as "3" or "3/12". Zero means "not set".
fn parse_number(value: &str) -> Option<u32> {
    let number: u32 = value.split('/').next()?.trim().parse().ok()?;
    (number > 0).then_some(number)
}

/// The front cover if there is one, otherwise the first picture of any type.
fn cover_picture<'a>(tags: &[&'a Tag]) -> Option<&'a Picture> {
    let pictures = || tags.iter().flat_map(|tag| tag.pictures());
    pictures()
        .find(|picture| picture.pic_type() == PictureType::CoverFront)
        .or_else(|| pictures().next())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn years() {
        assert_eq!(parse_year("2019"), Some(2019));
        assert_eq!(parse_year("2019-05-01T10:00:00Z"), Some(2019));
        assert_eq!(parse_year("05/01/2019"), Some(2019));
        assert_eq!(parse_year("May 1st, 1999"), Some(1999));
        assert_eq!(parse_year("20190501"), Some(2019));
        assert_eq!(parse_year("123456"), None);
        assert_eq!(parse_year("19"), None);
        assert_eq!(parse_year(""), None);
    }

    #[test]
    fn track_numbers() {
        assert_eq!(parse_number("3"), Some(3));
        assert_eq!(parse_number("03/12"), Some(3));
        assert_eq!(parse_number(" 7 / 9"), Some(7));
        assert_eq!(parse_number("0"), None);
        assert_eq!(parse_number("/12"), None);
        assert_eq!(parse_number("A1"), None);
    }

    #[test]
    fn unstorable_durations_count_as_unknown() {
        assert_eq!(storable_millis(Duration::from_millis(215_000)), 215_000);
        assert_eq!(storable_millis(Duration::from_secs(u64::MAX)), 0);
        assert_eq!(storable_millis(Duration::from_millis(u64::MAX)), 0);
    }

    #[test]
    fn untagged_track_uses_file_name() {
        let track = untagged_track(Path::new("/music/Some Song.mka"), 10, 20);
        assert_eq!(track.title, "Some Song");
        assert_eq!(track.format, AudioFormat::Unknown);
        assert_eq!((track.file_size, track.modified), (10, 20));
        let caf = untagged_track(Path::new("/music/x.caf"), 0, 0);
        assert_eq!(caf.format, AudioFormat::Alac);
    }
}
