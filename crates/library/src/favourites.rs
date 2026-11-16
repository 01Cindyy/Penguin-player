//! Favourite songs. Like playlist entries they are stored by file path, so they survive
//! rescans, and a favourite whose file disappears points at its track again once the
//! file is back. When a scan sees a file move, its favourite follows it (see `scanner`).

use std::path::{Path, PathBuf};

use mp_core::util::unix_now_millis;
use mp_core::{CoverId, Favourite, Favourites, TrackId};
use rusqlite::{Connection, OptionalExtension};

use crate::{Result, paths};

/// Every favourite, newest first, each resolved to the library track with the same
/// path (if any), and the image chosen for them.
pub(crate) fn all(conn: &Connection) -> Result<Favourites> {
    let mut statement = conn.prepare_cached(
        "SELECT favourite.path, track.id, favourite.added
         FROM favourites AS favourite
         LEFT JOIN tracks AS track ON track.path = favourite.path
         ORDER BY favourite.added DESC, favourite.path",
    )?;
    let entries = statement
        .query_map([], |row| {
            Ok(Favourite {
                path: paths::from_bytes(row.get(0)?),
                track: row.get::<_, Option<i64>>(1)?.map(TrackId),
                added: row.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    let image = conn
        .prepare_cached("SELECT image FROM favourites_image WHERE id = 0")?
        .query_row([], |row| row.get::<_, String>(0))
        .optional()?
        .map(CoverId);
    Ok(Favourites { entries, image })
}

/// Makes `tracks` favourites, the first of them newest, all newer than any favourite
/// so far. Tracks that already are favourites keep their place, and ids that aren't in
/// the library are skipped.
pub(crate) fn add(conn: &Connection, tracks: &[TrackId]) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    let newest: Option<i64> = tx
        .prepare_cached("SELECT MAX(added) FROM favourites")?
        .query_row([], |row| row.get(0))?;
    let count = tracks.len() as i64;
    let first = unix_now_millis().max(newest.map_or(0, |newest| newest + count));
    {
        let mut insert = tx.prepare_cached(
            "INSERT OR IGNORE INTO favourites (path, added)
             SELECT path, ?2 FROM tracks WHERE id = ?1",
        )?;
        for (offset, track) in tracks.iter().enumerate() {
            insert.execute((track.0, first - offset as i64))?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// Stops the songs at `paths` being favourites. Paths that aren't favourites are
/// ignored.
pub(crate) fn remove(conn: &Connection, remove: &[PathBuf]) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    {
        let mut delete = tx.prepare_cached("DELETE FROM favourites WHERE path = ?1")?;
        for path in remove {
            delete.execute([paths::to_bytes(path)])?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// Puts back favourites as they were, with the time they were first added, e.g. to
/// undo removing them. Ones that are favourites again by now are left as they are.
pub(crate) fn restore(conn: &Connection, favourites: &[Favourite]) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    {
        let mut insert =
            tx.prepare_cached("INSERT OR IGNORE INTO favourites (path, added) VALUES (?1, ?2)")?;
        for favourite in favourites {
            insert.execute((paths::to_bytes(&favourite.path), favourite.added))?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// Sets or, with `None`, clears the image chosen for the favourites.
pub(crate) fn set_image(conn: &Connection, image: Option<&CoverId>) -> Result<()> {
    match image {
        Some(image) => conn
            .prepare_cached("INSERT OR REPLACE INTO favourites_image (id, image) VALUES (0, ?1)")?
            .execute([image.0.as_str()])?,
        None => conn
            .prepare_cached("DELETE FROM favourites_image")?
            .execute([])?,
    };
    Ok(())
}

/// Makes the favourite with a moved file's old path use its new one.
pub(crate) fn repoint(conn: &Connection, from: &Path, to: &Path) -> Result<()> {
    conn.prepare_cached("UPDATE OR IGNORE favourites SET path = ?2 WHERE path = ?1")?
        .execute((paths::to_bytes(from), paths::to_bytes(to)))?;
    Ok(())
}
