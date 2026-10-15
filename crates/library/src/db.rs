//! Opening the SQLite database and keeping its schema up to date.

use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use rusqlite::{Connection, ErrorCode, TransactionBehavior};

use crate::{LibraryError, Result};

/// Schema migrations, applied in order. `PRAGMA user_version` records how many have run,
/// so a schema change is made by appending a migration, never by editing an applied one.
const MIGRATIONS: &[&str] = &[SCHEMA_V1, PLAYLIST_ITEM_PATHS_V2, FILE_CHANGE_DETAILS_V3];

/// Paths are BLOBs in the encoding of [`crate::paths`]. Ids use AUTOINCREMENT so the id
/// of a deleted row is never reused while the UI may still hold it.
const SCHEMA_V1: &str = "
CREATE TABLE tracks (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    path          BLOB    NOT NULL UNIQUE,
    title         TEXT    NOT NULL,
    artist        TEXT    NOT NULL,
    album         TEXT    NOT NULL,
    album_artist  TEXT    NOT NULL,
    genre         TEXT    NOT NULL,
    year          INTEGER,
    track_number  INTEGER,
    disc_number   INTEGER,
    duration_ms   INTEGER NOT NULL,
    format        TEXT    NOT NULL,
    sample_rate   INTEGER,
    bit_depth     INTEGER,
    channels      INTEGER,
    bitrate_kbps  INTEGER,
    file_size     INTEGER NOT NULL,
    modified      INTEGER NOT NULL,
    added         INTEGER NOT NULL,
    cover         TEXT
);

CREATE TABLE playlists (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    name      TEXT    NOT NULL,
    created   INTEGER NOT NULL,
    modified  INTEGER NOT NULL
);

CREATE TABLE playlist_items (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    playlist_id  INTEGER NOT NULL REFERENCES playlists (id) ON DELETE CASCADE,
    position     INTEGER NOT NULL,
    path         BLOB    NOT NULL
);

CREATE INDEX playlist_items_order ON playlist_items (playlist_id, position);
";

/// Lets the scanner find the playlist entries of a moved file by its old path.
const PLAYLIST_ITEM_PATHS_V2: &str = "
CREATE INDEX playlist_items_path ON playlist_items (path);
";

/// What scans need to notice changes that a file's size and its modification time in
/// whole seconds miss: the sub-second part of that time, and a fingerprint of the folder
/// images the track's cover may come from (NULL when it has an embedded cover).
/// Rows from before this migration have no `modified_nanos`, so the next scan reads
/// their files again, which fills both columns in.
const FILE_CHANGE_DETAILS_V3: &str = "
ALTER TABLE tracks ADD COLUMN modified_nanos INTEGER;
ALTER TABLE tracks ADD COLUMN folder_images INTEGER;
";

/// How long a statement waits for the other connection (UI or scanner) to release its lock.
const BUSY_TIMEOUT: Duration = Duration::from_secs(10);

/// Opens the database file, creating its folder if needed.
pub(crate) fn open(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(path)?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    enable_wal(&conn)?;
    // NORMAL is safe in WAL mode and avoids a sync on every commit.
    conn.execute_batch("PRAGMA synchronous = NORMAL;")?;
    prepare(conn)
}

/// Switches the database to WAL mode, which lets the UI read while the scanner writes.
/// The mode is stored in the file, so only the very first open does any work. Switching
/// needs an exclusive lock that SQLite doesn't wait for, so it is retried while busy.
fn enable_wal(conn: &Connection) -> Result<()> {
    let journal_mode = |conn: &Connection| -> rusqlite::Result<String> {
        conn.pragma_query_value(None, "journal_mode", |row| row.get(0))
    };
    if journal_mode(conn)?.eq_ignore_ascii_case("wal") {
        return Ok(());
    }
    let started = Instant::now();
    loop {
        match conn
            .pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get::<_, String>(0))
        {
            Ok(_) => return Ok(()),
            Err(err) if is_busy(&err) && started.elapsed() < BUSY_TIMEOUT => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(err) => return Err(err.into()),
        }
    }
}

fn is_busy(err: &rusqlite::Error) -> bool {
    err.sqlite_error_code() == Some(ErrorCode::DatabaseBusy)
}

pub(crate) fn open_in_memory() -> Result<Connection> {
    prepare(Connection::open_in_memory()?)
}

fn prepare(mut conn: Connection) -> Result<Connection> {
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;
    conn.set_prepared_statement_cache_capacity(32);
    // Taking the write lock when a transaction starts (rather than on its first write)
    // means a busy database is waited on instead of failing halfway through.
    conn.set_transaction_behavior(TransactionBehavior::Immediate);
    migrate(&mut conn)?;
    Ok(conn)
}

/// Brings the schema up to date.
fn migrate(conn: &mut Connection) -> Result<()> {
    if schema_version(conn)? == MIGRATIONS.len() {
        return Ok(());
    }
    // Checked again under the write lock: when the UI and the scanner open a new
    // database at the same time, only the first may create the tables.
    let tx = conn.transaction()?;
    let applied = schema_version(&tx)?;
    if applied > MIGRATIONS.len() {
        return Err(LibraryError::Invalid(format!(
            "the library database uses schema version {applied}, which is newer than this \
             version of the app supports ({})",
            MIGRATIONS.len()
        )));
    }
    for migration in &MIGRATIONS[applied..] {
        tx.execute_batch(migration)?;
    }
    tx.pragma_update(None, "user_version", MIGRATIONS.len())?;
    tx.commit()?;
    Ok(())
}

fn schema_version(conn: &Connection) -> Result<usize> {
    Ok(conn.pragma_query_value(None, "user_version", |row| row.get(0))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_opens_of_a_new_database_all_succeed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/library.db");
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let path = path.clone();
                std::thread::spawn(move || open(&path).map(|_| ()))
            })
            .collect();
        for handle in handles {
            handle.join().unwrap().unwrap();
        }
        let conn = open(&path).unwrap();
        assert_eq!(schema_version(&conn).unwrap(), MIGRATIONS.len());
        let mode: String = conn
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
    }

    #[test]
    fn older_databases_are_migrated_and_keep_their_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(MIGRATIONS[0]).unwrap();
            conn.pragma_update(None, "user_version", 1).unwrap();
            conn.execute(
                "INSERT INTO playlists (name, created, modified) VALUES ('Kept', 0, 0)",
                [],
            )
            .unwrap();
        }
        let conn = open(&path).unwrap();
        assert_eq!(schema_version(&conn).unwrap(), MIGRATIONS.len());
        let name: String = conn
            .query_row("SELECT name FROM playlists", [], |row| row.get(0))
            .unwrap();
        assert_eq!(name, "Kept");
        let has_path_index: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE name = 'playlist_items_path'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(has_path_index);
    }

    #[test]
    fn newer_schemas_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.db");
        open(&path)
            .unwrap()
            .pragma_update(None, "user_version", MIGRATIONS.len() + 1)
            .unwrap();
        assert!(matches!(open(&path), Err(LibraryError::Invalid(_))));
    }

    #[test]
    fn a_reader_can_read_while_a_writer_holds_a_transaction() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.db");
        let mut writer = open(&path).unwrap();
        let reader = open(&path).unwrap();
        let tx = writer.transaction().unwrap();
        tx.execute(
            "INSERT INTO playlists (name, created, modified) VALUES ('x', 0, 0)",
            [],
        )
        .unwrap();
        let count: i64 = reader
            .query_row("SELECT COUNT(*) FROM playlists", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
        tx.commit().unwrap();
    }
}
