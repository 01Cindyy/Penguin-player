//! Track rows: loading, saving, and the per-file state the scanner compares against.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use mp_core::collection::library_order;
use mp_core::{AudioFormat, CoverId, Track, TrackId};
use rusqlite::{Connection, OptionalExtension, Row, named_params};

use crate::{Result, paths};

/// Selects every column in the order [`track_from_row`] reads them.
const SELECT_TRACKS: &str = "SELECT id, path, title, artist, album, album_artist, genre, year,
    track_number, disc_number, duration_ms, format, sample_rate, bit_depth, channels,
    bitrate_kbps, file_size, modified, added, cover FROM tracks";

/// Selects what [`stored_file_from_row`] reads.
const SELECT_FILES: &str =
    "SELECT path, id, file_size, modified, modified_nanos, folder_images, cover FROM tracks";

/// Inserts a track, or updates the row with the same path while keeping its id and the
/// time it was first added.
const UPSERT_TRACK: &str = "INSERT INTO tracks (path, title, artist, album, album_artist, genre,
    year, track_number, disc_number, duration_ms, format, sample_rate, bit_depth, channels,
    bitrate_kbps, file_size, modified, modified_nanos, folder_images, added, cover)
VALUES (:path, :title, :artist, :album, :album_artist, :genre, :year, :track_number,
    :disc_number, :duration_ms, :format, :sample_rate, :bit_depth, :channels, :bitrate_kbps,
    :file_size, :modified, :modified_nanos, :folder_images, :added, :cover)
ON CONFLICT (path) DO UPDATE SET
    title = excluded.title,
    artist = excluded.artist,
    album = excluded.album,
    album_artist = excluded.album_artist,
    genre = excluded.genre,
    year = excluded.year,
    track_number = excluded.track_number,
    disc_number = excluded.disc_number,
    duration_ms = excluded.duration_ms,
    format = excluded.format,
    sample_rate = excluded.sample_rate,
    bit_depth = excluded.bit_depth,
    channels = excluded.channels,
    bitrate_kbps = excluded.bitrate_kbps,
    file_size = excluded.file_size,
    modified = excluded.modified,
    modified_nanos = excluded.modified_nanos,
    folder_images = excluded.folder_images,
    cover = excluded.cover";

/// A track as the scanner saves it, with what later scans need to notice that its file
/// changed (see `disk::DiskFile`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ScannedTrack {
    pub track: Track,
    pub modified_nanos: u32,
    /// The folder image fingerprint when the track's cover may come from a folder image,
    /// or `None` if it has an embedded cover, which folder images can't change.
    pub folder_images: Option<i64>,
}

/// What the database knows about a track's file, enough to tell whether it changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredFile {
    pub id: TrackId,
    pub size: u64,
    pub modified: i64,
    /// `None` for tracks saved before sub-second times were stored.
    pub modified_nanos: Option<u32>,
    pub folder_images: Option<i64>,
    pub cover: Option<CoverId>,
}

/// Every track in the library's default order (see [`library_order`]).
pub(crate) fn all(conn: &Connection) -> Result<Vec<Track>> {
    let mut statement = conn.prepare_cached(SELECT_TRACKS)?;
    let mut tracks = statement
        .query_map([], track_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    tracks.sort_by_cached_key(library_order);
    Ok(tracks)
}

pub(crate) fn by_id(conn: &Connection, id: TrackId) -> Result<Option<Track>> {
    let mut statement = conn.prepare_cached(&format!("{SELECT_TRACKS} WHERE id = ?1"))?;
    Ok(statement.query_row([id.0], track_from_row).optional()?)
}

pub(crate) fn count(conn: &Connection) -> Result<usize> {
    Ok(conn.query_row("SELECT COUNT(*) FROM tracks", [], |row| row.get(0))?)
}

/// Whether any track's file is inside the folder `dir`.
pub(crate) fn any_inside(conn: &Connection, dir: &Path) -> Result<bool> {
    let (start, end) = paths::children_range(dir);
    let mut statement =
        conn.prepare_cached("SELECT EXISTS (SELECT 1 FROM tracks WHERE path >= ?1 AND path < ?2)")?;
    Ok(statement.query_row((start, end), |row| row.get(0))?)
}

/// The cover images that tracks use.
pub(crate) fn covers_in_use(conn: &Connection) -> Result<HashSet<String>> {
    let mut statement =
        conn.prepare_cached("SELECT DISTINCT cover FROM tracks WHERE cover IS NOT NULL")?;
    let covers = statement
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(covers)
}

pub(crate) fn upsert(conn: &Connection, scanned: &ScannedTrack) -> Result<()> {
    let track = &scanned.track;
    let mut statement = conn.prepare_cached(UPSERT_TRACK)?;
    statement.execute(named_params! {
        ":path": paths::to_bytes(&track.path),
        ":title": track.title,
        ":artist": track.artist,
        ":album": track.album,
        ":album_artist": track.album_artist,
        ":genre": track.genre,
        ":year": track.year,
        ":track_number": track.track_number,
        ":disc_number": track.disc_number,
        ":duration_ms": track.duration_ms,
        ":format": track.format.id(),
        ":sample_rate": track.sample_rate,
        ":bit_depth": track.bit_depth,
        ":channels": track.channels,
        ":bitrate_kbps": track.bitrate_kbps,
        ":file_size": track.file_size,
        ":modified": track.modified,
        ":modified_nanos": scanned.modified_nanos,
        ":folder_images": scanned.folder_images,
        ":added": track.added,
        ":cover": track.cover.as_ref().map(|cover| cover.0.as_str()),
    })?;
    Ok(())
}

/// Points a track at the new path of its moved file.
pub(crate) fn set_path(conn: &Connection, id: TrackId, path: &Path) -> Result<()> {
    conn.prepare_cached("UPDATE tracks SET path = ?2 WHERE id = ?1")?
        .execute((id.0, paths::to_bytes(path)))?;
    Ok(())
}

/// Deletes every track, returning how many there were.
pub(crate) fn delete_all(conn: &Connection) -> Result<usize> {
    Ok(conn.execute("DELETE FROM tracks", [])?)
}

pub(crate) fn delete(conn: &Connection, id: TrackId) -> Result<()> {
    conn.prepare_cached("DELETE FROM tracks WHERE id = ?1")?
        .execute([id.0])?;
    Ok(())
}

/// The stored file state of every track, by path.
pub(crate) fn all_files(conn: &Connection) -> Result<HashMap<PathBuf, StoredFile>> {
    let mut statement = conn.prepare_cached(SELECT_FILES)?;
    let files = statement
        .query_map([], stored_file_from_row)?
        .collect::<rusqlite::Result<_>>()?;
    Ok(files)
}

/// The stored file state of the track at `path`, and of every track inside `path` if it
/// is (or was) a folder.
pub(crate) fn files_at(conn: &Connection, path: &Path) -> Result<Vec<(PathBuf, StoredFile)>> {
    let (start, end) = paths::children_range(path);
    let mut statement = conn.prepare_cached(&format!(
        "{SELECT_FILES} WHERE path = ?1 OR (path >= ?2 AND path < ?3)"
    ))?;
    let files = statement
        .query_map((paths::to_bytes(path), start, end), stored_file_from_row)?
        .collect::<rusqlite::Result<_>>()?;
    Ok(files)
}

fn stored_file_from_row(row: &Row) -> rusqlite::Result<(PathBuf, StoredFile)> {
    let file = StoredFile {
        id: TrackId(row.get(1)?),
        size: row.get(2)?,
        modified: row.get(3)?,
        modified_nanos: row.get(4)?,
        folder_images: row.get(5)?,
        cover: row.get::<_, Option<String>>(6)?.map(CoverId),
    };
    Ok((paths::from_bytes(row.get(0)?), file))
}

fn track_from_row(row: &Row) -> rusqlite::Result<Track> {
    let format: String = row.get(11)?;
    Ok(Track {
        id: TrackId(row.get(0)?),
        path: paths::from_bytes(row.get(1)?),
        title: row.get(2)?,
        artist: row.get(3)?,
        album: row.get(4)?,
        album_artist: row.get(5)?,
        genre: row.get(6)?,
        year: row.get(7)?,
        track_number: row.get(8)?,
        disc_number: row.get(9)?,
        duration_ms: row.get(10)?,
        format: AudioFormat::from_id(&format),
        sample_rate: row.get(12)?,
        bit_depth: row.get(13)?,
        channels: row.get(14)?,
        bitrate_kbps: row.get(15)?,
        file_size: row.get(16)?,
        modified: row.get(17)?,
        added: row.get(18)?,
        cover: row.get::<_, Option<String>>(19)?.map(CoverId),
    })
}
