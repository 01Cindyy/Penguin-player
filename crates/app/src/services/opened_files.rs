//! Songs opened from a file manager ("Open With") or named on the command line. They
//! replace the queue and the first one plays. Songs in the library play as themselves;
//! others are read the way the scanner reads songs and play without joining the
//! library: the store keeps them until the app quits ([`LibraryStore::add_opened`]),
//! and their covers go to a folder of their own (`AppPaths::opened_covers_dir`).

use std::cell::Cell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gio, glib};
use mp_core::{Track, TrackId};

use crate::context::AppContext;
use crate::model::LibraryStore;

/// How long opening waits for the library to load, so that its songs play as library
/// songs. If it doesn't load, they play as songs from outside it.
const LIBRARY_WAIT: Duration = Duration::from_secs(10);

thread_local! {
    /// Counts openings, so that when they overlap only the latest one plays.
    static LATEST: Cell<u64> = const { Cell::new(0) };
}

/// What an opened path turned out to be.
enum Song {
    /// A library track, or a song opened before.
    Known(TrackId),
    /// A song from outside the library, read just now.
    Read(Box<Track>),
    /// Not a song that can be played: another kind of file, a folder or a broken file.
    Unplayable(PathBuf),
}

/// Plays `files`, in order, in place of the queue.
pub fn open(ctx: &Rc<AppContext>, files: &[gio::File]) {
    let mut paths = Vec::new();
    for file in files {
        match file.path() {
            Some(path) => paths.push(path),
            None => {
                log::warn!("Can’t open {}, which isn’t on this computer", file.uri());
                ctx.toast("Only songs on this computer can be played");
            }
        }
    }
    if paths.is_empty() {
        return;
    }
    let opening = LATEST.with(|latest| {
        latest.set(latest.get() + 1);
        latest.get()
    });
    let weak = Rc::downgrade(ctx);
    let store = ctx.store.clone();
    glib::spawn_future_local(async move {
        library_loaded(&store).await;
        let Some(ctx) = weak.upgrade() else { return };
        let mut known = store.track_paths();
        for path in &paths {
            let path = mp_library::normalize_path(path);
            if let Some(id) = store.opened_at(&path) {
                known.insert(path, id);
            }
        }
        let root = ctx.settings().music_folder.clone();
        let covers_dir = ctx.paths.opened_covers_dir();
        drop(ctx);
        let found =
            gio::spawn_blocking(move || find_songs(&paths, &known, root.as_deref(), &covers_dir))
                .await;
        let Some(ctx) = weak.upgrade() else { return };
        if LATEST.with(Cell::get) != opening {
            return;
        }
        match found {
            Ok(songs) => play(&ctx, songs),
            Err(_) => {
                log::error!("Reading the opened songs panicked");
                ctx.toast("Couldn’t open the songs");
            }
        }
    });
}

/// Waits until the library has loaded, for at most [`LIBRARY_WAIT`].
async fn library_loaded(store: &LibraryStore) {
    if store.loaded() {
        return;
    }
    let (sender, receiver) = async_channel::bounded(1);
    let handler = store.connect_loaded_notify(move |_| {
        let _ = sender.try_send(());
    });
    let _ = glib::future_with_timeout(LIBRARY_WAIT, receiver.recv()).await;
    store.disconnect(handler);
}

fn play(ctx: &AppContext, songs: Vec<Song>) {
    let mut ids = Vec::new();
    let mut unplayable = Vec::new();
    for song in songs {
        match song {
            Song::Known(id) => ids.push(id),
            Song::Read(track) => {
                if let Some(cover) = &track.cover {
                    ctx.covers.add_opened(cover);
                }
                ids.push(ctx.store.add_opened(*track));
            }
            Song::Unplayable(path) => unplayable.push(path),
        }
    }
    match unplayable.as_slice() {
        [] => {}
        [path] => {
            let name = path.file_name().unwrap_or(path.as_os_str());
            ctx.toast(&format!("Couldn’t play “{}”", name.to_string_lossy()));
        }
        paths => ctx.toast(&format!("Couldn’t play {} of the files", paths.len())),
    }
    if !ids.is_empty() {
        ctx.player.play_tracks(ids, 0);
    }
}

/// Finds the song at each path: a library track or a song opened before (both in
/// `known`, by path), or else one read now, with its cover written to `covers_dir`.
/// Runs off the main thread, as it reads files.
fn find_songs(
    paths: &[PathBuf],
    known: &HashMap<PathBuf, TrackId>,
    root: Option<&Path>,
    covers_dir: &Path,
) -> Vec<Song> {
    let root = root.map(mp_library::normalize_path);
    let real_root = root.as_ref().and_then(|root| root.canonicalize().ok());
    let mut songs = Vec::with_capacity(paths.len());
    let mut unread = Vec::new();
    for path in paths {
        let path = mp_library::normalize_path(path);
        match known_id(&path, known, root.as_deref(), real_root.as_deref()) {
            Some(id) => songs.push(Some(Song::Known(id))),
            None => {
                unread.push((songs.len(), path));
                songs.push(None);
            }
        }
    }
    let unread_paths: Vec<PathBuf> = unread.iter().map(|(_, path)| path.clone()).collect();
    let read = mp_library::read_songs(&unread_paths, covers_dir).unwrap_or_else(|err| {
        log::warn!("Could not read the opened songs: {err}");
        vec![None; unread_paths.len()]
    });
    for ((index, path), track) in unread.into_iter().zip(read) {
        songs[index] = Some(match track {
            Some(track) => Song::Read(Box::new(track)),
            None => Song::Unplayable(path),
        });
    }
    songs.into_iter().flatten().collect()
}

/// The id of the library track or opened song at `path`. A music folder reached
/// through a symlink has two spellings, so the file's real path is tried inside the
/// music folder too.
fn known_id(
    path: &Path,
    known: &HashMap<PathBuf, TrackId>,
    root: Option<&Path>,
    real_root: Option<&Path>,
) -> Option<TrackId> {
    if let Some(&id) = known.get(path) {
        return Some(id);
    }
    let real = path.canonicalize().ok()?;
    if let Some(&id) = known.get(&real) {
        return Some(id);
    }
    let inside = real.strip_prefix(real_root?).ok()?;
    known.get(&root?.join(inside)).copied()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn library_songs_are_found_through_a_symlinked_music_folder() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("drive/Music");
        std::fs::create_dir_all(real.join("Album")).unwrap();
        std::fs::write(real.join("Album/song.flac"), b"").unwrap();
        let root = dir.path().join("Music");
        std::os::unix::fs::symlink(&real, &root).unwrap();
        let known = HashMap::from([(root.join("Album/song.flac"), TrackId(7))]);
        let real_root = real.canonicalize().unwrap();

        let find = |path: &Path| known_id(path, &known, Some(&root), Some(&real_root));
        assert_eq!(find(&root.join("Album/song.flac")), Some(TrackId(7)));
        assert_eq!(find(&real.join("Album/song.flac")), Some(TrackId(7)));
        assert_eq!(find(&real.join("Album/other.flac")), None);
        assert_eq!(
            known_id(&real.join("Album/song.flac"), &known, None, None),
            None
        );
    }
}
