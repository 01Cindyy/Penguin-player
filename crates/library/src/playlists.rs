//! User playlists. Entries are stored by file path, so they survive rescans, and an entry
//! whose file disappears points at its track again once the file is back. When a scan
//! sees a file move, the entries follow it (see `scanner`).
//!
//! Positions within a playlist are always contiguous, starting at 0.

use std::collections::HashMap;
use std::path::Path;

use mp_core::search::normalize;
use mp_core::util::unix_now;
use mp_core::{Playlist, PlaylistEntry, PlaylistId, PlaylistItemId, TrackId};
use rusqlite::{Connection, OptionalExtension, Row};

use crate::{LibraryError, Result, paths};

/// All playlists sorted by name (ignoring case and accents), with their entries.
pub(crate) fn all(conn: &Connection) -> Result<Vec<Playlist>> {
    let mut statement = conn.prepare_cached("SELECT id, name, created, modified FROM playlists")?;
    let mut playlists = statement
        .query_map([], playlist_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut entries = all_entries(conn)?;
    for playlist in &mut playlists {
        playlist.entries = entries.remove(&playlist.id).unwrap_or_default();
    }
    playlists.sort_by_cached_key(|playlist| (normalize(&playlist.name), playlist.id));
    Ok(playlists)
}

pub(crate) fn by_id(conn: &Connection, id: PlaylistId) -> Result<Option<Playlist>> {
    let mut statement =
        conn.prepare_cached("SELECT id, name, created, modified FROM playlists WHERE id = ?1")?;
    let Some(mut playlist) = statement.query_row([id.0], playlist_from_row).optional()? else {
        return Ok(None);
    };
    playlist.entries = entries(conn, id)?;
    Ok(Some(playlist))
}

pub(crate) fn create(conn: &Connection, name: &str) -> Result<PlaylistId> {
    let name = valid_name(name)?;
    conn.prepare_cached("INSERT INTO playlists (name, created, modified) VALUES (?1, ?2, ?2)")?
        .execute((name, unix_now()))?;
    Ok(PlaylistId(conn.last_insert_rowid()))
}

pub(crate) fn rename(conn: &Connection, id: PlaylistId, name: &str) -> Result<()> {
    let name = valid_name(name)?;
    let changed = conn
        .prepare_cached("UPDATE playlists SET name = ?2, modified = ?3 WHERE id = ?1")?
        .execute((id.0, name, unix_now()))?;
    expect_playlist(changed, id)
}

pub(crate) fn delete(conn: &Connection, id: PlaylistId) -> Result<()> {
    // Its items go with it (ON DELETE CASCADE).
    let changed = conn
        .prepare_cached("DELETE FROM playlists WHERE id = ?1")?
        .execute([id.0])?;
    expect_playlist(changed, id)
}

/// Appends tracks in the given order. Fails without changing anything if a track
/// isn't in the library.
pub(crate) fn add_tracks(conn: &Connection, id: PlaylistId, tracks: &[TrackId]) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    touch(&tx, id)?;
    let start: usize = tx
        .prepare_cached("SELECT COUNT(*) FROM playlist_items WHERE playlist_id = ?1")?
        .query_row([id.0], |row| row.get(0))?;
    {
        let mut insert = tx.prepare_cached(
            "INSERT INTO playlist_items (playlist_id, position, path)
             SELECT ?1, ?2, path FROM tracks WHERE id = ?3",
        )?;
        for (offset, track) in tracks.iter().enumerate() {
            if insert.execute((id.0, start + offset, track.0))? == 0 {
                return Err(LibraryError::Invalid(format!(
                    "track {} is not in the library",
                    track.0
                )));
            }
        }
    }
    tx.commit()?;
    Ok(())
}

/// Removes entries. Items that aren't in this playlist are ignored.
pub(crate) fn remove_items(
    conn: &Connection,
    id: PlaylistId,
    remove: &[PlaylistItemId],
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    touch(&tx, id)?;
    {
        let mut delete =
            tx.prepare_cached("DELETE FROM playlist_items WHERE playlist_id = ?1 AND id = ?2")?;
        for item in remove {
            delete.execute((id.0, item.0))?;
        }
    }
    save_order(&tx, &items(&tx, id)?)?;
    tx.commit()?;
    Ok(())
}

/// Puts the entries in the order of `order`, which must list every entry of the
/// playlist exactly once. Fails without changing anything otherwise.
pub(crate) fn reorder(conn: &Connection, id: PlaylistId, order: &[PlaylistItemId]) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    touch(&tx, id)?;
    let mut stored: HashMap<PlaylistItemId, usize> = items(&tx, id)?.into_iter().collect();
    // Taking each entry out as it is listed catches unknown and repeated ones alike.
    let reordered: Option<Vec<(PlaylistItemId, usize)>> = order
        .iter()
        .map(|&item| stored.remove(&item).map(|position| (item, position)))
        .collect();
    match reordered {
        Some(reordered) if stored.is_empty() => save_order(&tx, &reordered)?,
        _ => {
            return Err(LibraryError::Invalid(format!(
                "a new order for playlist {} must list each of its entries once",
                id.0
            )));
        }
    }
    tx.commit()?;
    Ok(())
}

fn valid_name(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.is_empty() {
        return Err(LibraryError::Invalid(
            "a playlist name can't be empty".to_owned(),
        ));
    }
    Ok(name)
}

fn expect_playlist(changed_rows: usize, id: PlaylistId) -> Result<()> {
    if changed_rows == 0 {
        return Err(LibraryError::Invalid(format!(
            "playlist {} does not exist",
            id.0
        )));
    }
    Ok(())
}

/// Marks a playlist as modified now. Fails if it doesn't exist.
fn touch(conn: &Connection, id: PlaylistId) -> Result<()> {
    let changed = conn
        .prepare_cached("UPDATE playlists SET modified = ?2 WHERE id = ?1")?
        .execute((id.0, unix_now()))?;
    expect_playlist(changed, id)
}

/// A playlist's items with their stored positions, in playlist order.
fn items(conn: &Connection, id: PlaylistId) -> Result<Vec<(PlaylistItemId, usize)>> {
    let mut statement = conn.prepare_cached(
        "SELECT id, position FROM playlist_items WHERE playlist_id = ?1 ORDER BY position",
    )?;
    let items = statement
        .query_map([id.0], |row| Ok((PlaylistItemId(row.get(0)?), row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(items)
}

/// Makes the positions match the order of `items`, writing only the ones that changed.
fn save_order(conn: &Connection, items: &[(PlaylistItemId, usize)]) -> Result<()> {
    let mut update =
        conn.prepare_cached("UPDATE playlist_items SET position = ?2 WHERE id = ?1")?;
    for (index, &(item, position)) in items.iter().enumerate() {
        if position != index {
            update.execute((item.0, index))?;
        }
    }
    Ok(())
}

/// Makes entries that point at a moved file's old path point at its new one.
pub(crate) fn repoint_entries(conn: &Connection, from: &Path, to: &Path) -> Result<()> {
    conn.prepare_cached("UPDATE playlist_items SET path = ?2 WHERE path = ?1")?
        .execute((paths::to_bytes(from), paths::to_bytes(to)))?;
    Ok(())
}

/// One playlist's entries in order, each resolved to the library track with the same
/// path (if any).
fn entries(conn: &Connection, id: PlaylistId) -> Result<Vec<PlaylistEntry>> {
    let mut statement = conn.prepare_cached(
        "SELECT item.id, item.path, track.id
         FROM playlist_items AS item
         LEFT JOIN tracks AS track ON track.path = item.path
         WHERE item.playlist_id = ?1
         ORDER BY item.position",
    )?;
    let entries = statement
        .query_map([id.0], entry_from_row)?
        .collect::<rusqlite::Result<_>>()?;
    Ok(entries)
}

/// The entries of every playlist, like [`entries`] but in one query.
fn all_entries(conn: &Connection) -> Result<HashMap<PlaylistId, Vec<PlaylistEntry>>> {
    let mut statement = conn.prepare_cached(
        "SELECT item.id, item.path, track.id, item.playlist_id
         FROM playlist_items AS item
         LEFT JOIN tracks AS track ON track.path = item.path
         ORDER BY item.playlist_id, item.position",
    )?;
    let mut entries: HashMap<PlaylistId, Vec<PlaylistEntry>> = HashMap::new();
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        entries
            .entry(PlaylistId(row.get(3)?))
            .or_default()
            .push(entry_from_row(row)?);
    }
    Ok(entries)
}

/// Reads the item id, path and track id from the first three columns.
fn entry_from_row(row: &Row) -> rusqlite::Result<PlaylistEntry> {
    Ok(PlaylistEntry {
        item: PlaylistItemId(row.get(0)?),
        path: paths::from_bytes(row.get(1)?),
        track: row.get::<_, Option<i64>>(2)?.map(TrackId),
    })
}

/// A playlist without its entries.
fn playlist_from_row(row: &Row) -> rusqlite::Result<Playlist> {
    Ok(Playlist {
        id: PlaylistId(row.get(0)?),
        name: row.get(1)?,
        created: row.get(2)?,
        modified: row.get(3)?,
        entries: Vec::new(),
    })
}
