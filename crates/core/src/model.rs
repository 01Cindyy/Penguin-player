use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::format::AudioFormat;
use crate::search;
use crate::util::format_count;

pub const UNKNOWN_ARTIST: &str = "Unknown Artist";
pub const UNKNOWN_ALBUM: &str = "Unknown Album";

/// Database id of a track. Stable for as long as the file keeps its path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TrackId(pub i64);

/// File name of an extracted cover image inside the covers cache directory,
/// e.g. `"9f86d081884c7d65.jpg"`. Resolve it with [`crate::AppPaths::cover_path`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CoverId(pub String);

/// One audio file in the library together with its tags and stream properties.
///
/// Text tags use an empty string for "unknown"; use the `display_*` helpers when
/// showing them. `title` is never empty: the scanner falls back to the file name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: TrackId,
    pub path: PathBuf,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub genre: String,
    pub year: Option<u32>,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub duration_ms: u64,
    pub format: AudioFormat,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u8>,
    pub channels: Option<u8>,
    pub bitrate_kbps: Option<u32>,
    pub file_size: u64,
    /// File modification time (Unix seconds), used to skip unchanged files on rescan.
    pub modified: i64,
    /// When the track was first added to the library (Unix seconds).
    pub added: i64,
    pub cover: Option<CoverId>,
}

impl Track {
    pub fn duration(&self) -> Duration {
        Duration::from_millis(self.duration_ms)
    }

    pub fn display_artist(&self) -> &str {
        non_empty_or(&self.artist, UNKNOWN_ARTIST)
    }

    pub fn display_album(&self) -> &str {
        non_empty_or(&self.album, UNKNOWN_ALBUM)
    }

    /// The artist an album is filed under: album artist if tagged, otherwise the track artist.
    pub fn display_album_artist(&self) -> &str {
        if self.album_artist.is_empty() {
            self.display_artist()
        } else {
            &self.album_artist
        }
    }

    /// Lossless files above CD quality (more than 16 bit or more than 48 kHz).
    pub fn is_hi_res(&self) -> bool {
        self.format.is_lossless()
            && (self.bit_depth.is_some_and(|bits| bits > 16)
                || self.sample_rate.is_some_and(|rate| rate > 48_000))
    }

    /// Short quality description such as "24-bit / 96 kHz" or "320 kbps".
    pub fn quality_label(&self) -> String {
        let rate = self.sample_rate.map(format_sample_rate);
        if self.format.is_lossless() {
            match (self.bit_depth, rate) {
                (Some(bits), Some(rate)) => format!("{bits}-bit / {rate}"),
                (None, Some(rate)) => rate,
                (Some(bits), None) => format!("{bits}-bit"),
                (None, None) => String::new(),
            }
        } else {
            match (self.bitrate_kbps, rate) {
                (Some(kbps), _) => format!("{kbps} kbps"),
                (None, Some(rate)) => rate,
                (None, None) => String::new(),
            }
        }
    }

    /// Everything known about the audio's quality, such as "24-bit · 96 kHz · 2,304 kbps".
    /// Mono and multichannel audio say so; stereo, the usual, doesn't.
    pub fn quality_details(&self) -> String {
        let channels = match self.channels {
            Some(1) => Some("Mono".to_owned()),
            Some(count) if count > 2 => Some(format!("{count} channels")),
            _ => None,
        };
        [
            self.bit_depth.map(|bits| format!("{bits}-bit")),
            self.sample_rate.map(format_sample_rate),
            self.bitrate_kbps
                .map(|kbps| format!("{} kbps", format_count(kbps as usize))),
            channels,
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ")
    }

    /// Normalised text that searches match against (see [`crate::SearchQuery`]).
    pub fn search_text(&self) -> String {
        search::normalize(&format!(
            "{} {} {} {} {} {}",
            self.title,
            self.artist,
            self.album,
            self.album_artist,
            self.genre,
            self.format.label()
        ))
    }
}

fn non_empty_or<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    if value.is_empty() { fallback } else { value }
}

fn format_sample_rate(rate: u32) -> String {
    if rate.is_multiple_of(1000) {
        format!("{} kHz", rate / 1000)
    } else {
        format!("{:.1} kHz", f64::from(rate) / 1000.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PlaylistId(pub i64);

/// Identifies one row of a playlist, so duplicates of the same track can be told apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PlaylistItemId(pub i64);

/// A user playlist. Entries are stored by file path so playlists survive rescans
/// and changes of the music folder; `track` is `None` while the file is missing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Playlist {
    pub id: PlaylistId,
    pub name: String,
    pub created: i64,
    pub modified: i64,
    pub entries: Vec<PlaylistEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlaylistEntry {
    pub item: PlaylistItemId,
    pub path: PathBuf,
    pub track: Option<TrackId>,
}

impl Playlist {
    /// Tracks that are currently present in the library, in playlist order.
    pub fn available_tracks(&self) -> impl Iterator<Item = TrackId> + '_ {
        self.entries.iter().filter_map(|entry| entry.track)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn track(id: i64, title: &str, artist: &str, album: &str) -> Track {
        Track {
            id: TrackId(id),
            path: PathBuf::from(format!("/music/{title}.flac")),
            title: title.to_owned(),
            artist: artist.to_owned(),
            album: album.to_owned(),
            album_artist: String::new(),
            genre: String::new(),
            year: None,
            track_number: None,
            disc_number: None,
            duration_ms: 180_000,
            format: AudioFormat::Flac,
            sample_rate: Some(44_100),
            bit_depth: Some(16),
            channels: Some(2),
            bitrate_kbps: Some(900),
            file_size: 20_000_000,
            modified: 0,
            added: 0,
            cover: None,
        }
    }

    #[test]
    fn quality_labels() {
        let mut t = track(1, "a", "b", "c");
        assert_eq!(t.quality_label(), "16-bit / 44.1 kHz");
        assert!(!t.is_hi_res());
        t.bit_depth = Some(24);
        t.sample_rate = Some(96_000);
        assert_eq!(t.quality_label(), "24-bit / 96 kHz");
        assert!(t.is_hi_res());
        t.format = AudioFormat::Mp3;
        t.bitrate_kbps = Some(320);
        assert_eq!(t.quality_label(), "320 kbps");
        assert!(!t.is_hi_res());
    }

    #[test]
    fn quality_details_list_everything_known() {
        let mut t = track(1, "a", "b", "c");
        t.bit_depth = Some(24);
        t.sample_rate = Some(96_000);
        t.bitrate_kbps = Some(2304);
        assert_eq!(t.quality_details(), "24-bit · 96 kHz · 2,304 kbps");
        t.format = AudioFormat::Mp3;
        t.bit_depth = None;
        t.sample_rate = Some(44_100);
        t.bitrate_kbps = Some(320);
        t.channels = Some(1);
        assert_eq!(t.quality_details(), "44.1 kHz · 320 kbps · Mono");
        t.channels = Some(6);
        t.sample_rate = None;
        t.bitrate_kbps = None;
        assert_eq!(t.quality_details(), "6 channels");
        t.channels = None;
        assert_eq!(t.quality_details(), "");
    }

    #[test]
    fn display_fallbacks() {
        let mut t = track(1, "Song", "", "");
        assert_eq!(t.display_artist(), UNKNOWN_ARTIST);
        assert_eq!(t.display_album(), UNKNOWN_ALBUM);
        assert_eq!(t.display_album_artist(), UNKNOWN_ARTIST);
        t.artist = "Solo".into();
        assert_eq!(t.display_album_artist(), "Solo");
        t.album_artist = "Band".into();
        assert_eq!(t.display_album_artist(), "Band");
    }
}
