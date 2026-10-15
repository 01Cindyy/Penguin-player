//! The music library: scanning a folder, reading tags with lofty, storing everything
//! in SQLite, user playlists and watching the folder for changes.
//!
//! `Library` wraps one SQLite connection and is not `Sync`. The app keeps one on the
//! UI thread for quick reads and playlist edits, and opens another on a worker thread
//! for scanning (the database runs in WAL mode so both can be used at once).

mod chapter_limit;
mod covers;
mod db;
mod disk;
mod lyrics;
mod paths;
mod playlists;
mod scanner;
mod tags;
mod tracks;
mod watcher;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use mp_core::{Playlist, PlaylistId, PlaylistItemId, Track, TrackId};
use rusqlite::Connection;

pub use lyrics::{read_found_lyrics, read_lyrics, save_found_lyrics};
pub use watcher::LibraryWatcher;

#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("file watcher error: {0}")]
    Watch(String),
    #[error("{0}")]
    Invalid(String),
    /// The music folder is missing, or holds no audio files while the library still has
    /// tracks. Most likely an unmounted drive, so the library is left untouched; call
    /// [`Library::clear_tracks`] if the music really is gone.
    #[error("the music folder {} looks empty or unavailable", .0.display())]
    FolderUnavailable(PathBuf),
    /// A scan was stopped by its caller (see [`Library::scan_cancellable`]).
    #[error("the scan was cancelled")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, LibraryError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanOptions {
    /// The music folder. Tracks outside it are removed from the library by a full scan.
    pub root: PathBuf,
    /// Where extracted cover art is written (see `AppPaths::covers_dir`). Full scans
    /// delete the images in it that no track uses any more, so it must belong to this
    /// library alone (files the cache didn't name are never touched).
    pub covers_dir: PathBuf,
    /// Re-read tags even for files whose size and modification time are unchanged.
    pub force: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanProgress {
    pub processed: usize,
    pub total: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanSummary {
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    pub unchanged: usize,
    pub failed: usize,
}

impl ScanSummary {
    pub fn changed(&self) -> bool {
        self.added + self.updated + self.removed > 0
    }
}

pub struct Library {
    conn: Connection,
}

impl Library {
    /// Opens (creating and migrating if needed) the database at `db_path`.
    pub fn open(db_path: &Path) -> Result<Self> {
        Ok(Self {
            conn: db::open(db_path)?,
        })
    }

    pub fn open_in_memory() -> Result<Self> {
        Ok(Self {
            conn: db::open_in_memory()?,
        })
    }

    /// Every track, ordered by album artist, album, disc and track number.
    pub fn tracks(&self) -> Result<Vec<Track>> {
        tracks::all(&self.conn)
    }

    pub fn track(&self, id: TrackId) -> Result<Option<Track>> {
        tracks::by_id(&self.conn, id)
    }

    pub fn track_count(&self) -> Result<usize> {
        tracks::count(&self.conn)
    }

    /// Scans the whole music folder: new and changed files are (re)read, unchanged files
    /// are skipped and tracks whose files are gone are removed. Blocking: run it on a
    /// worker thread, and one scan at a time. `progress` is called from worker threads
    /// as files are processed.
    ///
    /// A file counts as changed when its size or modification time (to the nanosecond,
    /// where the file system records it) differs, or when a folder image its cover could
    /// come from (`cover.jpg` etc.) was added, changed or removed. Tracks in folders that
    /// can't be read, or behind a symlink whose target is missing, are kept.
    ///
    /// A file that was moved or renamed along with its folder (same name, size and
    /// modification time) keeps its track id, `added` time and playlist entries, and
    /// counts as updated. Changes are saved in batches as the scan goes, so the UI's
    /// connection is never locked out for long and an interrupted scan keeps its work.
    ///
    /// Track paths are stored absolute and normalised (`/music/./a//b.flac` is
    /// `/music/a/b.flac`). Fails with [`LibraryError::FolderUnavailable`] without touching
    /// the library if the music folder doesn't exist or has no audio files while the
    /// library has tracks, so an unmounted drive doesn't empty the library.
    pub fn scan(
        &mut self,
        options: &ScanOptions,
        progress: &(dyn Fn(ScanProgress) + Sync),
    ) -> Result<ScanSummary> {
        scanner::scan(&mut self.conn, options, progress, &|| false)
    }

    /// Like [`Library::scan`], but stops with [`LibraryError::Cancelled`] soon after
    /// `cancelled` returns true, for example because the user chose another folder.
    /// Changes saved before that are kept; the next full scan completes or undoes them.
    /// `cancelled` is called from worker threads too.
    pub fn scan_cancellable(
        &mut self,
        options: &ScanOptions,
        progress: &(dyn Fn(ScanProgress) + Sync),
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<ScanSummary> {
        scanner::scan(&mut self.conn, options, progress, cancelled)
    }

    /// Re-checks specific files or folders, e.g. those reported by [`LibraryWatcher`].
    /// Paths that no longer exist are removed from the library, and moves are detected
    /// as in [`Library::scan`] when both the old and the new path are given.
    ///
    /// A folder image re-checks the tracks in its folder. Paths outside the music folder
    /// (including ones that climb out of it with `..`) are ignored, and tracks outside the
    /// given paths are never touched. Fails with [`LibraryError::FolderUnavailable`] if
    /// the music folder is missing, or empty while the library has tracks in it.
    pub fn scan_paths(&mut self, options: &ScanOptions, paths: &[PathBuf]) -> Result<ScanSummary> {
        scanner::scan_paths(&mut self.conn, options, paths)
    }

    /// Removes every track, for when the user confirms their music really is gone after
    /// a scan reported [`LibraryError::FolderUnavailable`]. Playlists are kept: their
    /// entries are stored by path and come back when the files do. Returns how many
    /// tracks were removed.
    pub fn clear_tracks(&mut self) -> Result<usize> {
        tracks::delete_all(&self.conn)
    }

    /// All playlists, sorted by name, with their entries resolved against the library.
    pub fn playlists(&self) -> Result<Vec<Playlist>> {
        playlists::all(&self.conn)
    }

    pub fn playlist(&self, id: PlaylistId) -> Result<Option<Playlist>> {
        playlists::by_id(&self.conn, id)
    }

    /// Creates an empty playlist. The name is trimmed and must not be empty.
    pub fn create_playlist(&self, name: &str) -> Result<PlaylistId> {
        playlists::create(&self.conn, name)
    }

    pub fn rename_playlist(&self, id: PlaylistId, name: &str) -> Result<()> {
        playlists::rename(&self.conn, id, name)
    }

    pub fn delete_playlist(&self, id: PlaylistId) -> Result<()> {
        playlists::delete(&self.conn, id)
    }

    /// Appends tracks to the end of a playlist.
    pub fn add_to_playlist(&self, id: PlaylistId, tracks: &[TrackId]) -> Result<()> {
        playlists::add_tracks(&self.conn, id, tracks)
    }

    pub fn remove_from_playlist(&self, id: PlaylistId, items: &[PlaylistItemId]) -> Result<()> {
        playlists::remove_items(&self.conn, id, items)
    }

    /// Reorders a whole playlist in one transaction. `order` must list every entry of
    /// the playlist exactly once; anything else is [`LibraryError::Invalid`] and
    /// changes nothing.
    pub fn reorder_playlist(&self, id: PlaylistId, order: &[PlaylistItemId]) -> Result<()> {
        playlists::reorder(&self.conn, id, order)
    }
}
