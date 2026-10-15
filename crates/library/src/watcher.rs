//! Watching the music folder for changes, reported in debounced batches.

use std::collections::HashSet;
use std::fs;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use notify::event::{
    AccessKind, AccessMode, CreateKind, EventKind, ModifyKind, RemoveKind, RenameMode,
};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher, WatcherKind};
use walkdir::WalkDir;

use crate::covers::is_folder_image;
use mp_core::files::is_hidden_name;

use crate::disk::{is_audio_file, is_hidden_below};
use crate::{LibraryError, Result, paths};

/// How long the folder must be quiet before a batch of changes is reported, so copying
/// an album gives one batch rather than one per file.
const DEBOUNCE: Duration = Duration::from_millis(1500);

/// A batch is reported this long after its first change even if changes keep coming,
/// so a file being written for minutes (a download, say) doesn't hold back the rest.
const MAX_BATCH_DELAY: Duration = Duration::from_secs(10);

/// Watches the music folder and everything in it. Dropping it stops watching.
pub struct LibraryWatcher {
    messages: Sender<Message>,
    /// Set once the folder itself was deleted or moved away.
    stopped: Arc<AtomicBool>,
}

enum Message {
    Event(notify::Result<Event>),
    Stop,
}

impl LibraryWatcher {
    /// `on_change` is called on a background thread with debounced batches of paths that
    /// were created, modified, renamed or deleted: supported audio files, folder images
    /// (`cover.jpg` etc.), folders, and paths that vanished (which may have been
    /// folders). Pass them to [`crate::Library::scan_paths`]. Hidden files and folders
    /// are left out, as scans leave them out.
    ///
    /// Fails with `Invalid` if `root` isn't an existing folder. Subfolders that can't be
    /// watched (usually for lack of permission) are skipped with a warning, as scans skip
    /// folders they can't read. If the folder is deleted or moved later, it is reported
    /// once and watching stops (see [`Self::is_watching`]); create a new watcher once it
    /// is back.
    pub fn new(root: &Path, on_change: impl Fn(Vec<PathBuf>) + Send + 'static) -> Result<Self> {
        let root = paths::normalize(root);
        if !root.is_dir() {
            return Err(LibraryError::Invalid(format!(
                "the music folder {} does not exist or is not a folder",
                root.display()
            )));
        }
        let (messages, received) = mpsc::channel();
        let events = messages.clone();
        let mut watcher = RecommendedWatcher::new(
            move |event| {
                let _ = events.send(Message::Event(event));
            },
            notify::Config::default(),
        )
        .map_err(watch_error)?;
        watch_folders(&mut watcher, &root).map_err(watch_error)?;

        let stopped = Arc::new(AtomicBool::new(false));
        let batcher = Batcher {
            watcher,
            root: WatchedRoot::new(root),
            stopped: stopped.clone(),
            on_change: Box::new(on_change),
            pending: Vec::new(),
            seen: HashSet::new(),
            first_change: None,
        };
        thread::Builder::new()
            .name("library watcher".into())
            .spawn(move || batcher.run(&received))
            .map_err(|error| LibraryError::Watch(error.to_string()))?;
        Ok(Self { messages, stopped })
    }

    /// Whether changes are still being followed: false once the folder itself has been
    /// deleted or moved away, after that was reported.
    pub fn is_watching(&self) -> bool {
        !self.stopped.load(Ordering::Relaxed)
    }
}

impl Drop for LibraryWatcher {
    fn drop(&mut self) {
        let _ = self.messages.send(Message::Stop);
    }
}

fn watch_error(error: notify::Error) -> LibraryError {
    LibraryError::Watch(error.to_string())
}

/// Whether the watcher has to watch every folder of a tree itself. inotify (Linux) needs
/// one watch per folder, and notify's recursive mode would add one for every folder in
/// the tree, hidden ones included: `.git`, `.Trash-1000`, or all of `~/.cache` when the
/// music folder is the home folder. Those cost kernel memory, count against a per-user
/// limit other apps share, and their changes would only be thrown away. So with inotify
/// the visible folders are watched one by one, the way scans walk them. The other
/// backends watch a whole tree with one native watch.
fn watches_each_folder() -> bool {
    RecommendedWatcher::kind() == WatcherKind::Inotify
}

/// Watches `dir` and the folders below it that scans look into. Fails only if `dir`
/// itself can't be watched; subfolders that can't be are skipped with a warning.
fn watch_folders(watcher: &mut RecommendedWatcher, dir: &Path) -> notify::Result<()> {
    if !watches_each_folder() {
        return watcher.watch(dir, RecursiveMode::Recursive);
    }
    for (index, folder) in visible_folders(dir).enumerate() {
        match watcher.watch(&folder, RecursiveMode::NonRecursive) {
            Ok(()) => {}
            Err(error) if index == 0 => return Err(error),
            Err(error) if matches!(error.kind, notify::ErrorKind::MaxFilesWatch) => {
                log::warn!(
                    "Watching only part of {}: {error}. Raising the system's limit on \
                     watched folders lets the rest be watched too.",
                    dir.display()
                );
                break;
            }
            Err(error) => {
                log::warn!("Not watching {} for changes: {error}", folder.display());
            }
        }
    }
    Ok(())
}

/// `dir` followed by every folder below it that isn't hidden or inside a hidden one.
/// Symlinks are followed, as scans follow them.
fn visible_folders(dir: &Path) -> impl Iterator<Item = PathBuf> {
    WalkDir::new(dir)
        .follow_links(true)
        .into_iter()
        .filter_entry(|entry| entry.depth() == 0 || !is_hidden_name(entry.file_name()))
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_dir())
        .map(walkdir::DirEntry::into_path)
}

/// Owns the notify watcher on the watcher thread: collects its events into batches,
/// watches folders that appear, and reports each batch once the folder is quiet.
struct Batcher {
    watcher: RecommendedWatcher,
    root: WatchedRoot,
    stopped: Arc<AtomicBool>,
    on_change: Box<dyn Fn(Vec<PathBuf>) + Send>,
    /// The events of the batch being collected, each once.
    pending: Vec<Event>,
    seen: HashSet<Event>,
    first_change: Option<Instant>,
}

impl Batcher {
    /// Runs until the [`LibraryWatcher`] is dropped. Sleeps until the first change of a
    /// batch arrives, so an idle watcher costs nothing.
    fn run(mut self, messages: &Receiver<Message>) {
        loop {
            let message = match self.first_change {
                None => messages.recv().map_err(|_| RecvTimeoutError::Disconnected),
                Some(first) if first.elapsed() >= MAX_BATCH_DELAY => {
                    self.report();
                    continue;
                }
                Some(_) => messages.recv_timeout(DEBOUNCE),
            };
            match message {
                Ok(Message::Event(Ok(event))) => self.add(event),
                Ok(Message::Event(Err(error))) => {
                    log::warn!("Error watching {}: {error}", self.root.path.display());
                }
                Err(RecvTimeoutError::Timeout) => self.report(),
                Ok(Message::Stop) | Err(RecvTimeoutError::Disconnected) => break,
            }
        }
    }

    fn add(&mut self, event: Event) {
        if watches_each_folder() {
            self.watch_if_new_folder(&event);
        }
        if !event.need_rescan() && !is_change(&event.kind) {
            return;
        }
        if self.seen.insert(event.clone()) {
            self.pending.push(event);
            self.first_change.get_or_insert_with(Instant::now);
        }
    }

    /// Watches a folder that was just created or moved into the library, with the
    /// folders inside it. After lost events, every folder is looked at again.
    fn watch_if_new_folder(&mut self, event: &Event) {
        let folder = match event.kind {
            _ if event.need_rescan() => Some(&self.root.path),
            EventKind::Create(_) | EventKind::Modify(ModifyKind::Name(RenameMode::To)) => {
                event.paths.first()
            }
            _ => None,
        };
        let Some(folder) =
            folder.filter(|folder| !is_hidden_below(&self.root.path, folder) && folder.is_dir())
        else {
            return;
        };
        if let Err(error) = watch_folders(&mut self.watcher, folder) {
            log::warn!("Not watching {} for changes: {error}", folder.display());
        }
    }

    fn report(&mut self) {
        self.first_change = None;
        self.seen.clear();
        let events = std::mem::take(&mut self.pending);
        if events.iter().any(|event| root_went_away(&self.root, event)) {
            self.stopped.store(true, Ordering::Relaxed);
        }
        let paths = changed_paths(&self.root, &events);
        if paths.is_empty() {
            return;
        }
        // A panicking callback must not take the watcher thread down with it.
        if panic::catch_unwind(AssertUnwindSafe(|| (self.on_change)(paths))).is_err() {
            log::error!("The library change handler panicked");
        }
    }
}

/// The music folder, under both of the names events may use for it.
struct WatchedRoot {
    /// The folder as given, which is how the library names its tracks.
    path: PathBuf,
    /// The folder with symlinks resolved, if that differs. macOS reports changes under
    /// this name, so a library behind a symlink (`~/Music` → `/Volumes/Data/Music`)
    /// would otherwise never hear of them.
    resolved: Option<PathBuf>,
}

impl WatchedRoot {
    fn new(path: PathBuf) -> Self {
        let resolved = fs::canonicalize(&path)
            .ok()
            .filter(|resolved| *resolved != path);
        Self { path, resolved }
    }

    /// An event's path as the library names it, or `None` if it is outside the folder.
    fn library_path(&self, path: &Path) -> Option<PathBuf> {
        if path.starts_with(&self.path) {
            return Some(path.to_path_buf());
        }
        let relative = path.strip_prefix(self.resolved.as_ref()?).ok()?;
        if relative.as_os_str().is_empty() {
            Some(self.path.clone())
        } else {
            Some(self.path.join(relative))
        }
    }
}

/// Whether `event` says the watched folder itself was deleted or moved. Its watches
/// go with it: a moved folder is still watched, but at its new place.
fn root_went_away(root: &WatchedRoot, event: &Event) -> bool {
    matches!(
        event.kind,
        EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(_))
    ) && event
        .paths
        .iter()
        .any(|path| root.library_path(path).as_deref() == Some(root.path.as_path()))
}

/// The paths in a batch of events that the library cares about, without duplicates.
fn changed_paths(root: &WatchedRoot, events: &[Event]) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut paths = Vec::new();
    let mut report = |path: PathBuf| {
        if seen.insert(path.clone()) {
            paths.push(path);
        }
    };

    for event in events {
        if event.need_rescan() {
            // Events were lost (e.g. the kernel's queue overflowed): re-check everything.
            report(root.path.clone());
            continue;
        }
        if !is_change(&event.kind) {
            continue;
        }
        for path in event
            .paths
            .iter()
            .filter_map(|path| root.library_path(path))
        {
            if !is_hidden_below(&root.path, &path) && may_hold_tracks(&path, &event.kind) {
                report(path);
            }
        }
    }
    paths
}

/// Whether an event can mean that a file's content or existence changed. Opening and
/// reading files (which the scanner itself does) doesn't.
fn is_change(kind: &EventKind) -> bool {
    match kind {
        EventKind::Access(AccessKind::Close(AccessMode::Write)) => true,
        EventKind::Access(_) => false,
        _ => true,
    }
}

/// Supported audio files, folder images (which may change the covers of the tracks next
/// to them), folders, and paths that no longer exist unless the event says they were
/// plain files. A vanished path can't be inspected, and a folder moved out of the library
/// (to the trash, say) must be reported or its tracks would stay behind; checking a path
/// that held no tracks is cheap for `scan_paths`.
fn may_hold_tracks(path: &Path, kind: &EventKind) -> bool {
    if is_audio_file(path) || is_folder_image(path) {
        return true;
    }
    match fs::metadata(path) {
        Ok(metadata) => metadata.is_dir(),
        Err(_) => !is_file_event(kind),
    }
}

/// Whether the event itself shows that its path is a file rather than a folder.
fn is_file_event(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(CreateKind::File)
            | EventKind::Remove(RemoveKind::File)
            | EventKind::Modify(ModifyKind::Data(_))
            | EventKind::Access(_)
    )
}

#[cfg(test)]
mod tests {
    use notify::event::{DataChange, Flag};

    use super::*;

    fn watched(root: &Path) -> WatchedRoot {
        WatchedRoot::new(root.to_path_buf())
    }

    fn event(kind: EventKind, paths: &[&Path]) -> Event {
        paths.iter().fold(Event::new(kind), |event, path| {
            event.add_path(path.to_path_buf())
        })
    }

    #[test]
    fn keeps_audio_files_folder_images_and_folders_once() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let album = root.join("Album");
        fs::create_dir(&album).unwrap();
        let song = album.join("01.flac");
        let cover = album.join("Cover.JPG");
        let gone_song = album.join("02.mp3");
        let gone_folder = root.join("Vol. 2");
        let events = [
            event(EventKind::Create(CreateKind::Folder), &[&album]),
            event(EventKind::Create(CreateKind::File), &[&song]),
            event(
                EventKind::Modify(ModifyKind::Data(DataChange::Any)),
                &[&song],
            ),
            event(EventKind::Create(CreateKind::File), &[&cover]),
            event(
                EventKind::Create(CreateKind::File),
                &[&album.join("back.jpg")],
            ),
            event(
                EventKind::Remove(RemoveKind::File),
                &[&album.join("notes.txt")],
            ),
            event(EventKind::Remove(RemoveKind::File), &[&gone_song]),
            event(EventKind::Remove(RemoveKind::Any), &[&gone_folder]),
            event(
                EventKind::Access(AccessKind::Open(AccessMode::Read)),
                &[&song],
            ),
            event(
                EventKind::Create(CreateKind::File),
                &[&root.join(".hidden/x.flac")],
            ),
            event(
                EventKind::Create(CreateKind::File),
                &[Path::new("/elsewhere/x.flac")],
            ),
        ];
        assert_eq!(
            changed_paths(&watched(root), &events),
            [album, song, cover, gone_song, gone_folder]
        );
    }

    #[test]
    fn renames_report_both_sides() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let (from, to) = (root.join("Old.2020"), root.join("New.2021"));
        fs::create_dir(&to).unwrap();
        let rename = event(
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
            &[&from, &to],
        );
        assert_eq!(changed_paths(&watched(root), &[rename]), [from, to]);
    }

    #[test]
    fn folders_moved_out_are_reported_whatever_their_name() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let moved_out = root.join("Album.2020");
        let renamed_temp_file = root.join("download.part");
        let events = [
            event(
                EventKind::Modify(ModifyKind::Name(RenameMode::From)),
                &[&moved_out],
            ),
            event(
                EventKind::Modify(ModifyKind::Data(DataChange::Content)),
                &[&renamed_temp_file],
            ),
        ];
        assert_eq!(changed_paths(&watched(root), &events), [moved_out]);
    }

    /// macOS names changes by the folder's resolved path; they are reported under the
    /// path the library uses for a music folder reached through a symlink.
    #[cfg(unix)]
    #[test]
    fn changes_named_by_the_resolved_folder_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("Data/Music");
        fs::create_dir_all(real.join("Album")).unwrap();
        let link = dir.path().join("Music");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let resolved = fs::canonicalize(&real).unwrap();

        let song = resolved.join("Album/01.flac");
        let events = [
            event(EventKind::Create(CreateKind::File), &[&song]),
            event(
                EventKind::Create(CreateKind::Folder),
                &[&resolved.join("Album")],
            ),
            event(EventKind::Remove(RemoveKind::Folder), &[&resolved]),
            event(
                EventKind::Create(CreateKind::File),
                &[&dir.path().join("Data/x.flac")],
            ),
        ];
        assert_eq!(
            changed_paths(&watched(&link), &events),
            [link.join("Album/01.flac"), link.join("Album"), link.clone()]
        );
    }

    #[test]
    fn hidden_folders_are_not_watched() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".music");
        for folder in ["Album/CD 1", ".git/objects/ab", "Album/.thumbnails/large"] {
            fs::create_dir_all(root.join(folder)).unwrap();
        }
        fs::write(root.join("Album/.hidden.flac"), "").unwrap();
        let mut folders: Vec<PathBuf> = visible_folders(&root).collect();
        folders.sort();
        assert_eq!(
            folders,
            [root.clone(), root.join("Album"), root.join("Album/CD 1")]
        );
    }

    #[test]
    fn lost_events_rescan_the_root() {
        let root = Path::new("/music");
        let lost = Event::new(EventKind::Other).set_flag(Flag::Rescan);
        let events = [lost];
        assert_eq!(changed_paths(&watched(root), &events), [root]);
    }
}
