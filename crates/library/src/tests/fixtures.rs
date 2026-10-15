//! Self-contained test fixtures: tiny WAV files written in code and tagged with lofty,
//! inside a temporary folder that also holds the database and covers cache.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use lofty::config::WriteOptions;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::prelude::*;
use lofty::tag::{Tag, TagType};
use mp_core::Track;
use tempfile::TempDir;

use crate::{Library, ScanOptions, ScanSummary};

/// A library database plus a music folder and covers cache, all in one temporary folder.
pub struct TestLibrary {
    _dir: TempDir,
    pub root: PathBuf,
    pub covers: PathBuf,
    pub library: Library,
}

impl TestLibrary {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().expect("create temp dir");
        let root = dir.path().join("music");
        fs::create_dir(&root).expect("create music folder");
        let library = Library::open(&dir.path().join("data/library.db")).expect("open library");
        Self {
            covers: dir.path().join("covers"),
            root,
            library,
            _dir: dir,
        }
    }

    pub fn options(&self) -> ScanOptions {
        ScanOptions {
            root: self.root.clone(),
            covers_dir: self.covers.clone(),
            force: false,
        }
    }

    pub fn scan(&mut self) -> ScanSummary {
        let options = self.options();
        self.library.scan(&options, &|_| {}).expect("scan")
    }

    pub fn scan_paths(&mut self, paths: &[PathBuf]) -> ScanSummary {
        let options = self.options();
        self.library
            .scan_paths(&options, paths)
            .expect("scan paths")
    }

    /// A path inside the music folder, with its parent folders created.
    pub fn path(&self, relative: &str) -> PathBuf {
        let path = self.root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        path
    }

    pub fn track_at(&self, path: &Path) -> Option<Track> {
        let tracks = self.library.tracks().expect("load tracks");
        tracks.into_iter().find(|track| track.path == path)
    }

    pub fn cover_files(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(&self.covers)
            .map(|entries| {
                entries
                    .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }
}

/// Tags to write into a test file. Empty fields are left out.
#[derive(Default, Clone)]
pub struct TestTags {
    pub kind: TagKind,
    pub title: &'static str,
    pub artist: &'static str,
    pub album: &'static str,
    pub album_artist: &'static str,
    pub date: &'static str,
    pub track: &'static str,
    pub disc: &'static str,
    /// Written as unsynced lyrics (ID3v2 `USLT`).
    pub lyrics: &'static str,
    pub pictures: Vec<(PictureType, Vec<u8>)>,
}

#[derive(Default, Clone, Copy)]
pub enum TagKind {
    #[default]
    Id3v2,
    RiffInfo,
}

/// Writes a short silent WAV (16-bit stereo at 44.1 kHz) with the given tags.
pub fn write_wav(path: &Path, tags: &TestTags) {
    fs::write(path, wav_bytes(44_100, 2, 16, 4_410)).unwrap();
    let mut tag = Tag::new(match tags.kind {
        TagKind::Id3v2 => TagType::Id3v2,
        TagKind::RiffInfo => TagType::RiffInfo,
    });
    // Lofty writes "3/12" style pairs (e.g. ID3v2 TRCK) from separate number and total.
    let (track, track_total) = tags.track.split_once('/').unwrap_or((tags.track, ""));
    let (disc, disc_total) = tags.disc.split_once('/').unwrap_or((tags.disc, ""));
    let fields = [
        (ItemKey::TrackTitle, tags.title),
        (ItemKey::TrackArtist, tags.artist),
        (ItemKey::AlbumTitle, tags.album),
        (ItemKey::AlbumArtist, tags.album_artist),
        (ItemKey::RecordingDate, tags.date),
        (ItemKey::TrackNumber, track),
        (ItemKey::TrackTotal, track_total),
        (ItemKey::DiscNumber, disc),
        (ItemKey::DiscTotal, disc_total),
        (ItemKey::UnsyncLyrics, tags.lyrics),
    ];
    for (key, value) in fields.into_iter().filter(|(_, value)| !value.is_empty()) {
        tag.insert_text(key, value.to_owned());
    }
    for (kind, data) in &tags.pictures {
        tag.push_picture(
            Picture::unchecked(data.clone())
                .pic_type(*kind)
                .mime_type(MimeType::Png)
                .build(),
        );
    }
    if !tag.is_empty() || !tags.pictures.is_empty() {
        tag.save_to_path(path, WriteOptions::new()).unwrap();
    }
}

/// A minimal PCM WAV file: RIFF header, `fmt ` chunk and `frames` frames of silence.
pub fn wav_bytes(sample_rate: u32, channels: u16, bits: u16, frames: u32) -> Vec<u8> {
    let block_align = channels * bits / 8;
    let data_len = frames * u32::from(block_align);
    let mut bytes = Vec::with_capacity(44 + data_len as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&channels.to_le_bytes());
    bytes.extend_from_slice(&sample_rate.to_le_bytes());
    bytes.extend_from_slice(&(sample_rate * u32::from(block_align)).to_le_bytes());
    bytes.extend_from_slice(&block_align.to_le_bytes());
    bytes.extend_from_slice(&bits.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    bytes.resize(bytes.len() + data_len as usize, 0);
    bytes
}

/// Bytes that start like a PNG; `seed` makes images with different content.
pub fn png(seed: u8) -> Vec<u8> {
    let mut data = b"\x89PNG\r\n\x1a\n".to_vec();
    data.extend_from_slice(&[seed; 32]);
    data
}

/// Bytes that start like a JPEG.
pub fn jpeg(seed: u8) -> Vec<u8> {
    let mut data = vec![0xFF, 0xD8, 0xFF, 0xE0];
    data.extend_from_slice(&[seed; 32]);
    data
}

/// Sets a file's modification time to `seconds` after the Unix epoch.
pub fn set_mtime(path: &Path, seconds: u64) {
    set_mtime_precisely(path, Duration::from_secs(seconds));
}

/// Sets a file's modification time to `since_epoch` after the Unix epoch.
pub fn set_mtime_precisely(path: &Path, since_epoch: Duration) {
    set_modified(path, SystemTime::UNIX_EPOCH + since_epoch);
}

pub fn set_modified(path: &Path, time: SystemTime) {
    let file = File::options().write(true).open(path).unwrap();
    file.set_modified(time).unwrap();
}
