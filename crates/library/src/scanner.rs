//! Scanning: compare the audio files on disk with what the database knows, read the tags
//! of new and changed files in parallel, then write the changes in batches.

use std::collections::HashMap;
use std::error::Error;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::num::NonZero;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::{fs, io, thread};

use mp_core::util::unix_now;
use mp_core::{Track, TrackId};
use rayon::prelude::*;
use rayon::{ThreadPool, ThreadPoolBuilder};
use rusqlite::Connection;

use crate::chapter_limit::TAG_READER_STACK_BYTES;
use crate::covers::{self, CoverStore};
use crate::disk::{DiskFile, DiskState, is_hidden_below};
use crate::tags::{self, FileInfo};
use crate::tracks::{self, ScannedTrack, StoredFile};
use crate::{LibraryError, Result, ScanOptions, ScanProgress, ScanSummary, paths, playlists};

/// How many files are read and then saved in one transaction. Small enough that the UI's
/// connection never waits long for the write lock, and that an interrupted first scan
/// keeps most of its work.
pub(crate) const WRITE_BATCH: usize = 500;

/// Scans the whole music folder. Every track in the database is in scope, so tracks
/// outside the folder are removed too.
pub(crate) fn scan(
    conn: &mut Connection,
    options: &ScanOptions,
    progress: &(dyn Fn(ScanProgress) + Sync),
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<ScanSummary> {
    let root = existing_root(options)?;
    let mut disk = DiskState::default();
    disk.walk(&root, cancelled);
    if cancelled() {
        return Err(LibraryError::Cancelled);
    }
    if disk.is_empty() && tracks::any_inside(conn, &root)? {
        return Err(LibraryError::FolderUnavailable(root));
    }
    let covers = CoverStore::open(&options.covers_dir)?;
    let stored = tracks::all_files(conn)?;
    let summary = sync(
        conn,
        options.force,
        &covers,
        disk,
        stored,
        progress,
        cancelled,
    )?;
    // Only a full scan has seen every track, so only it knows which covers are unused.
    covers.remove_unused(&tracks::covers_in_use(conn)?);
    Ok(summary)
}

/// Scans only the given files and folders. Only tracks at or inside those paths are in
/// scope; paths outside the music folder are ignored.
pub(crate) fn scan_paths(
    conn: &mut Connection,
    options: &ScanOptions,
    targets: &[PathBuf],
) -> Result<ScanSummary> {
    let root = existing_root(options)?;
    // Telling whether the folder still has audio files would mean walking all of it, so
    // here only an entirely empty folder counts as unavailable.
    if is_empty_dir(&root) && tracks::any_inside(conn, &root)? {
        return Err(LibraryError::FolderUnavailable(root));
    }
    let covers = CoverStore::open(&options.covers_dir)?;
    let mut disk = DiskState::default();
    let mut stored = HashMap::new();
    for target in targets.iter().map(|target| paths::normalize(target)) {
        if !paths::is_inside(&root, &target) {
            log::debug!("Ignoring {} outside the music folder", target.display());
            continue;
        }
        // A folder image can change the cover of every track in its folder.
        let target = match target.parent() {
            Some(dir) if covers::is_folder_image(&target) && paths::is_inside(&root, dir) => {
                dir.to_path_buf()
            }
            _ => target,
        };
        stored.extend(tracks::files_at(conn, &target)?);
        // Hidden paths are left out of the library, so any tracks there are removed.
        if !is_hidden_below(&root, &target) {
            disk.add_path(&target);
        }
    }
    sync(conn, options.force, &covers, disk, stored, &|_| {}, &|| {
        false
    })
}

/// The normalised music folder. A missing folder, like an empty one while the library
/// has tracks in it, is far more likely an unmounted drive than a deleted collection, so
/// scans refuse to run rather than remove every track.
fn existing_root(options: &ScanOptions) -> Result<PathBuf> {
    let root = paths::normalize(&options.root);
    if root.is_dir() {
        Ok(root)
    } else {
        Err(LibraryError::FolderUnavailable(root))
    }
}

fn is_empty_dir(dir: &Path) -> bool {
    fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_none())
}

/// Why a file's tags are being read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Change {
    New,
    Modified,
    /// A stored track's file now lives at another path.
    Moved,
}

/// A file whose tags need reading.
struct PendingFile {
    file: DiskFile,
    change: Change,
}

/// A stored track whose file now lives at another path.
struct Move {
    id: TrackId,
    from: PathBuf,
    file: DiskFile,
}

/// Brings the database in line with `disk`. `stored` must hold the database's state of
/// exactly the files and folders that were looked at on disk.
fn sync(
    conn: &mut Connection,
    force: bool,
    covers: &CoverStore,
    disk: DiskState,
    stored: HashMap<PathBuf, StoredFile>,
    progress: &(dyn Fn(ScanProgress) + Sync),
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<ScanSummary> {
    let mut diff = Diff::new(disk, stored, covers, force);
    let moves = take_moves(&mut diff.new, &mut diff.gone);
    let removed: Vec<TrackId> = diff.gone.iter().map(|(_, file)| file.id).collect();
    write_moves_and_removals(conn, &moves, &removed)?;

    let mut summary = ScanSummary {
        unchanged: diff.unchanged,
        removed: removed.len(),
        ..ScanSummary::default()
    };
    // Moved files are read again like modified ones, which also refreshes a cover that
    // came from the old folder's image.
    let pending: Vec<PendingFile> = diff
        .modified
        .into_iter()
        .map(|file| PendingFile {
            file,
            change: Change::Modified,
        })
        .chain(moves.into_iter().map(|moved| PendingFile {
            file: moved.file,
            change: Change::Moved,
        }))
        .chain(diff.new.into_iter().map(|file| PendingFile {
            file,
            change: Change::New,
        }))
        .collect();
    read_and_save(conn, &pending, covers, progress, cancelled, &mut summary)?;
    Ok(summary)
}

/// How the files found on disk compare with the stored tracks.
#[derive(Default)]
struct Diff {
    unchanged: usize,
    modified: Vec<DiskFile>,
    new: Vec<DiskFile>,
    /// Stored tracks whose files weren't found (and aren't in an unreadable folder).
    gone: Vec<(PathBuf, StoredFile)>,
}

impl Diff {
    fn new(
        mut disk: DiskState,
        mut stored: HashMap<PathBuf, StoredFile>,
        covers: &CoverStore,
        force: bool,
    ) -> Self {
        let mut diff = Self::default();
        for file in disk.take_files() {
            match stored.remove(&file.path) {
                Some(known) if !force && is_unchanged(&file, &known, covers) => {
                    diff.unchanged += 1;
                }
                Some(_) => diff.modified.push(file),
                None => diff.new.push(file),
            }
        }
        diff.gone = stored
            .into_iter()
            .filter(|(path, _)| !disk.is_unreadable(path))
            .collect();
        diff
    }
}

fn is_unchanged(file: &DiskFile, stored: &StoredFile, covers: &CoverStore) -> bool {
    file.size == stored.size
        && file.modified == stored.modified
        && stored.modified_nanos == Some(file.modified_nanos)
        // A folder image was added, changed or removed, so the cover may have changed.
        && stored
            .folder_images
            .is_none_or(|fingerprint| fingerprint == file.folder_images)
        // A cover deleted from the cache is extracted again by re-reading the file.
        && stored.cover.as_ref().is_none_or(|cover| covers.contains(cover))
}

/// Reads the pending files' tags in parallel and saves them, one batch at a time. Once
/// `cancelled`, the files left are skipped and the batch being read isn't saved.
fn read_and_save(
    conn: &mut Connection,
    pending: &[PendingFile],
    covers: &CoverStore,
    progress: &(dyn Fn(ScanProgress) + Sync),
    cancelled: &(dyn Fn() -> bool + Sync),
    summary: &mut ScanSummary,
) -> Result<()> {
    let reporter = ProgressReporter::new(
        progress,
        summary.unchanged,
        summary.unchanged + pending.len(),
    );
    if pending.is_empty() {
        return Ok(());
    }
    let readers = tag_reader_pool(pending.len())?;
    for batch in pending.chunks(WRITE_BATCH) {
        let mut save = Vec::with_capacity(batch.len());
        let scanned = readers.install(|| read_files(batch, covers, &reporter, cancelled));
        if cancelled() {
            return Err(LibraryError::Cancelled);
        }
        for (pending, scanned) in batch.iter().zip(scanned) {
            match (scanned, pending.change) {
                (Some(scanned), Change::New) => {
                    summary.added += 1;
                    save.push(scanned);
                }
                (Some(scanned), Change::Modified | Change::Moved) => {
                    summary.updated += 1;
                    save.push(scanned);
                }
                // The track was already moved to its new path; it keeps its old tags.
                (None, Change::Moved) => summary.updated += 1,
                (None, Change::New | Change::Modified) => summary.failed += 1,
            }
        }
        write_tracks(conn, &save)?;
    }
    Ok(())
}

/// Identifies a file across a move: its name, size and modification time, which moving
/// a file or renaming its folder keeps.
type MoveKey = (OsString, u64, i64, Option<u32>);

fn disk_move_key(file: &DiskFile) -> MoveKey {
    let name = file.path.file_name().unwrap_or_default().to_owned();
    (name, file.size, file.modified, Some(file.modified_nanos))
}

fn stored_move_key(path: &Path, stored: &StoredFile) -> MoveKey {
    let name = path.file_name().unwrap_or_default().to_owned();
    (name, stored.size, stored.modified, stored.modified_nanos)
}

/// Takes the files that were moved out of `new` and `gone`: a new file and a vanished
/// track with the same [`MoveKey`], each the only one with that key on its side.
/// Keeping the track's row (rather than removing it and adding a new one) keeps its id,
/// the time it was added and the playlist entries that point at it.
fn take_moves(new: &mut Vec<DiskFile>, gone: &mut Vec<(PathBuf, StoredFile)>) -> Vec<Move> {
    if new.is_empty() || gone.is_empty() {
        return Vec::new();
    }
    let mut new_counts: HashMap<MoveKey, usize> = HashMap::new();
    for file in new.iter() {
        *new_counts.entry(disk_move_key(file)).or_default() += 1;
    }
    let mut gone_by_key: HashMap<MoveKey, Vec<(PathBuf, StoredFile)>> = HashMap::new();
    for (path, stored) in gone.drain(..) {
        gone_by_key
            .entry(stored_move_key(&path, &stored))
            .or_default()
            .push((path, stored));
    }

    let mut moves = Vec::new();
    let mut still_new = Vec::with_capacity(new.len());
    for file in new.drain(..) {
        let key = disk_move_key(&file);
        match gone_by_key.get_mut(&key) {
            Some(candidates) if candidates.len() == 1 && new_counts[&key] == 1 => {
                let (from, stored) = candidates.pop().expect("exactly one candidate");
                moves.push(Move {
                    id: stored.id,
                    from,
                    file,
                });
            }
            _ => still_new.push(file),
        }
    }
    *new = still_new;
    gone.extend(gone_by_key.into_values().flatten());
    moves
}

/// Threads to read the tags of `files` files on, one per CPU at most, with the stack
/// that `tags::read` needs.
fn tag_reader_pool(files: usize) -> Result<ThreadPool> {
    let threads = thread::available_parallelism().map_or(1, NonZero::get);
    ThreadPoolBuilder::new()
        .num_threads(threads.min(files))
        .stack_size(TAG_READER_STACK_BYTES)
        .thread_name(|index| format!("tag-reader-{index}"))
        .build()
        .map_err(|error| LibraryError::Io(io::Error::other(error)))
}

/// Reads tags in parallel. Returns each file's track in order, or `None` if it failed.
fn read_files(
    batch: &[PendingFile],
    covers: &CoverStore,
    progress: &ProgressReporter,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Vec<Option<ScannedTrack>> {
    let added = unix_now();
    batch
        .par_iter()
        .map(|pending| {
            if cancelled() {
                return None;
            }
            let scanned = read_file(&pending.file, covers, added);
            progress.advance();
            scanned
        })
        .collect()
}

/// Reads one file's tags and cover. A panic inside a tag parser is caught, so one broken
/// file can't take the scan (or the app) down; it is logged and counted as failed.
fn read_file(file: &DiskFile, covers: &CoverStore, added: i64) -> Option<ScannedTrack> {
    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        tags::read(&file.path, file.size, file.modified)
            .map(|info| scanned_track(file, info, covers, added))
    }));
    match result {
        Ok(Ok(scanned)) => Some(scanned),
        Ok(Err(err)) => {
            log::warn!(
                "Could not read tags of {}: {}",
                file.path.display(),
                with_causes(&err)
            );
            None
        }
        Err(_) => {
            log::error!("Tag reader panicked on {}", file.path.display());
            None
        }
    }
}

/// Completes what was read from a file with its cover and what later scans compare.
fn scanned_track(file: &DiskFile, info: FileInfo, covers: &CoverStore, added: i64) -> ScannedTrack {
    let embedded = info
        .picture
        .as_ref()
        .and_then(|picture| covers.store(picture.data()));
    // Without an embedded cover the folder's images decide it, so their state is kept
    // to notice when they change.
    let (cover, folder_images) = match embedded {
        Some(cover) => (Some(cover), None),
        None => {
            let folder_cover = file.path.parent().and_then(|dir| covers.folder_cover(dir));
            (folder_cover, Some(file.folder_images))
        }
    };
    ScannedTrack {
        track: Track {
            added,
            cover,
            ..info.track
        },
        modified_nanos: file.modified_nanos,
        folder_images,
    }
}

/// An error message followed by its causes, since lofty's own message only names the
/// file type ("failed to parse Flac file: ...").
fn with_causes(error: &dyn Error) -> String {
    let mut text = error.to_string();
    let mut cause = error.source();
    while let Some(error) = cause {
        let _ = write!(text, ": {error}");
        cause = error.source();
    }
    text
}

/// Moves tracks to their new paths (taking their playlist entries along) and deletes
/// removed tracks, in one transaction.
fn write_moves_and_removals(
    conn: &mut Connection,
    moves: &[Move],
    removed: &[TrackId],
) -> Result<()> {
    if moves.is_empty() && removed.is_empty() {
        return Ok(());
    }
    let tx = conn.transaction()?;
    for moved in moves {
        tracks::set_path(&tx, moved.id, &moved.file.path)?;
        playlists::repoint_entries(&tx, &moved.from, &moved.file.path)?;
    }
    for &id in removed {
        tracks::delete(&tx, id)?;
    }
    tx.commit()?;
    Ok(())
}

/// Saves new and changed tracks in one transaction.
fn write_tracks(conn: &mut Connection, scanned: &[ScannedTrack]) -> Result<()> {
    if scanned.is_empty() {
        return Ok(());
    }
    let tx = conn.transaction()?;
    for track in scanned {
        tracks::upsert(&tx, track)?;
    }
    tx.commit()?;
    Ok(())
}

/// Reports progress from the reader threads. Updates are serialised so the callback
/// sees `processed` count up one at a time.
struct ProgressReporter<'a> {
    callback: &'a (dyn Fn(ScanProgress) + Sync),
    state: Mutex<ScanProgress>,
}

impl<'a> ProgressReporter<'a> {
    fn new(callback: &'a (dyn Fn(ScanProgress) + Sync), processed: usize, total: usize) -> Self {
        let state = ScanProgress { processed, total };
        callback(state);
        Self {
            callback,
            state: Mutex::new(state),
        }
    }

    fn advance(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.processed += 1;
        (self.callback)(*state);
    }
}
