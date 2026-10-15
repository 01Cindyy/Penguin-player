use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use super::fixtures::{TestTags, write_wav};
use crate::{LibraryError, LibraryWatcher};

/// Starts watching `root`, sending each reported batch to the returned receiver.
fn watch(root: &Path) -> (LibraryWatcher, Receiver<Vec<PathBuf>>) {
    let (sender, receiver) = mpsc::channel();
    let watcher = LibraryWatcher::new(root, move |paths| {
        let _ = sender.send(paths);
    })
    .unwrap();
    (watcher, receiver)
}

/// Collects reported paths until all of `expected` show up, failing after a few seconds.
fn wait_for(receiver: &Receiver<Vec<PathBuf>>, expected: &[&Path]) -> Vec<PathBuf> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut reported: Vec<PathBuf> = Vec::new();
    while let Some(missing) = expected
        .iter()
        .find(|path| !reported.iter().any(|seen| seen == *path))
    {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let batch = receiver
            .recv_timeout(remaining)
            .unwrap_or_else(|_| panic!("{} was not reported", missing.display()));
        reported.extend(batch);
    }
    reported
}

#[test]
fn reports_a_new_file() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let (_watcher, receiver) = watch(&root);

    let song = root.join("new song.wav");
    write_wav(&song, &TestTags::default());
    fs::write(root.join("notes.txt"), "ignored").unwrap();

    let reported = wait_for(&receiver, &[&song]);
    assert!(!reported.contains(&root.join("notes.txt")));
}

/// A folder whose name looks like a file name ("Album.2020") is reported once it has
/// been moved away, so its tracks don't stay in the library.
#[test]
fn reports_a_folder_moved_out_of_the_library() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("music");
    let album = root.join("Album.2020");
    fs::create_dir_all(&album).unwrap();
    write_wav(&album.join("01.wav"), &TestTags::default());
    let (_watcher, receiver) = watch(&root);

    fs::rename(&album, dir.path().join("trash.2020")).unwrap();
    wait_for(&receiver, &[&album]);
}

#[test]
fn the_root_must_be_an_existing_folder() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("song.wav");
    write_wav(&file, &TestTags::default());
    for root in [file, dir.path().join("missing")] {
        assert!(matches!(
            LibraryWatcher::new(&root, |_| {}),
            Err(LibraryError::Invalid(_))
        ));
    }
}

/// The library keeps its tracks when the whole folder goes away (see
/// `Library::scan_paths`), but it has to hear about it first.
#[test]
fn reports_the_music_folder_itself_when_it_is_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("music");
    fs::create_dir_all(root.join("Album")).unwrap();
    write_wav(&root.join("Album/01.wav"), &TestTags::default());
    let (watcher, receiver) = watch(&root);
    assert!(watcher.is_watching());

    fs::remove_dir_all(&root).unwrap();
    wait_for(&receiver, &[&root]);
    assert!(!watcher.is_watching());
}

/// A folder moved away and replaced (a drive swapped, a folder restored from a backup)
/// is reported, and the watcher says it no longer follows the folder at that path.
#[test]
fn reports_the_music_folder_itself_when_it_is_moved_away() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("music");
    fs::create_dir_all(root.join("Album")).unwrap();
    let (watcher, receiver) = watch(&root);

    fs::rename(&root, dir.path().join("old music")).unwrap();
    fs::create_dir(&root).unwrap();
    wait_for(&receiver, &[&root]);
    assert!(!watcher.is_watching());
}

/// A subfolder that can't be watched is skipped; the rest of the folder still is.
#[cfg(unix)]
#[test]
fn unreadable_subfolders_do_not_stop_the_rest_being_watched() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let locked = root.join("Locked");
    fs::create_dir(&locked).unwrap();
    fs::create_dir(root.join("Open")).unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let (sender, receiver) = mpsc::channel();
    let watcher = LibraryWatcher::new(&root, move |paths| {
        let _ = sender.send(paths);
    });
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    let _watcher = watcher.unwrap();

    let in_root = root.join("in root.wav");
    let in_subfolder = root.join("Open/in subfolder.wav");
    write_wav(&in_root, &TestTags::default());
    write_wav(&in_subfolder, &TestTags::default());
    wait_for(&receiver, &[&in_root, &in_subfolder]);
}

/// Folders created while watching are watched too, down to the ones created inside
/// them, so later changes in them are reported.
#[test]
fn reports_changes_in_folders_created_while_watching() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let (_watcher, receiver) = watch(&root);

    let disc = root.join("New Album/Disc 1");
    fs::create_dir_all(&disc).unwrap();
    wait_for(&receiver, &[&root.join("New Album")]);

    let song = disc.join("01.wav");
    write_wav(&song, &TestTags::default());
    wait_for(&receiver, &[&song]);
}

/// Hidden folders are neither scanned nor watched.
#[test]
fn changes_in_hidden_folders_are_not_reported() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    fs::create_dir_all(root.join(".git/objects")).unwrap();
    let (_watcher, receiver) = watch(&root);

    write_wav(&root.join(".git/objects/01.wav"), &TestTags::default());
    fs::create_dir(root.join(".Trash-1000")).unwrap();
    write_wav(&root.join(".Trash-1000/02.wav"), &TestTags::default());
    let song = root.join("visible.wav");
    write_wav(&song, &TestTags::default());

    let reported = wait_for(&receiver, &[&song]);
    assert_eq!(reported, [song]);
}
