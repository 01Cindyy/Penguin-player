//! Loading the library, scanning the music folder and watching it for changes.
//!
//! One worker thread owns its own [`Library`] connection and runs [`Job`]s in order,
//! so scans never overlap. Results come back to the main thread as [`Update`]s.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gio, glib};
use mp_core::{PlayerEvent, Playlist};
use mp_library::{Library, LibraryError, LibraryWatcher, ScanOptions, ScanProgress, ScanSummary};

use crate::context::AppContext;
use crate::model::LibrarySnapshot;
use crate::util::{count_label, song_count};

/// Scan progress is reported to the UI at most this often.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

enum Job {
    /// Read everything from the database.
    Load,
    /// Scan the whole music folder, unless the user has chosen another one since:
    /// `folder` is the [`FolderGeneration`] the scan was asked for.
    Scan { options: ScanOptions, folder: u64 },
    /// Re-check paths reported by the folder watcher.
    Changed(Vec<PathBuf>),
    /// Watch `options.root` (replacing any previous watcher), or stop with `None`.
    Watch(Option<ScanOptions>),
    /// Remove every track, after the user confirmed their music is really gone.
    ClearTracks,
}

enum Update {
    Library {
        snapshot: LibrarySnapshot,
        playlists: Vec<Playlist>,
    },
    ScanStarted,
    ScanProgress(ScanProgress),
    ScanFinished(ScanSummary),
    /// The music folder is missing, or empty while the library has tracks in it (often
    /// an unplugged drive). The scan changed nothing.
    FolderUnavailable(PathBuf),
    Error(String),
}

mod imp {
    use std::cell::Cell;

    use super::*;

    #[derive(Default, glib::Properties)]
    #[properties(wrapper_type = super::LibraryService)]
    pub struct LibraryService {
        pub(super) jobs: RefCell<Option<mpsc::Sender<Job>>>,
        pub(super) context: RefCell<Weak<AppContext>>,
        pub(super) folder: FolderGeneration,
        /// Followed while the music folder is missing (see `follow_mounts`).
        pub(super) mounts: RefCell<Option<(gio::VolumeMonitor, glib::SignalHandlerId)>>,

        /// True while a scan is running.
        #[property(get)]
        pub(super) scanning: Cell<bool>,
        /// Files processed by the running scan.
        #[property(get)]
        pub(super) scan_processed: Cell<u32>,
        /// Files the running scan will process (0 until known).
        #[property(get)]
        pub(super) scan_total: Cell<u32>,
        /// True once a scan finds the music folder missing or empty (see
        /// [`mp_library::LibraryError::FolderUnavailable`]), until a scan succeeds.
        /// The library is left untouched meanwhile.
        #[property(get)]
        pub(super) folder_missing: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for LibraryService {
        const NAME: &'static str = "MpLibraryService";
        type Type = super::LibraryService;
    }

    #[glib::derived_properties]
    impl ObjectImpl for LibraryService {}
}

glib::wrapper! {
    /// Keeps the [`crate::model::LibraryStore`] in sync with the database and the music folder.
    pub struct LibraryService(ObjectSubclass<imp::LibraryService>);
}

impl Default for LibraryService {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl LibraryService {
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts the worker: loads the library, then scans and watches the music folder.
    pub fn start(&self, ctx: &Rc<AppContext>) {
        let (jobs, job_receiver) = mpsc::channel();
        let (updates, update_receiver) = async_channel::unbounded();
        let db_path = ctx.paths.library_db();
        let watcher_jobs = jobs.clone();
        let folder = self.imp().folder.clone();
        let spawned = std::thread::Builder::new()
            .name("library".into())
            .spawn(move || run_worker(&db_path, &job_receiver, watcher_jobs, folder, updates));
        if let Err(err) = spawned {
            log::error!("Could not start the library thread: {err}");
            ctx.toast("Couldn’t load the music library");
            return;
        }

        self.imp().jobs.replace(Some(jobs));
        self.imp().context.replace(Rc::downgrade(ctx));
        self.receive_updates(update_receiver);
        self.send(Job::Load);
        self.rescan(false);
        self.update_watcher();
    }

    /// Scans the music folder again. With `force`, unchanged files are re-read too.
    pub fn rescan(&self, force: bool) {
        if let Some(options) = self.scan_options(force) {
            let folder = self.imp().folder.current();
            self.send(Job::Scan { options, folder });
        }
    }

    /// Switches to a new music folder: saves the setting, moves the watcher and scans
    /// the folder (tracks outside it are dropped).
    pub fn set_music_folder(&self, folder: PathBuf) {
        let Some(ctx) = self.context() else { return };
        ctx.update_settings(|settings| settings.music_folder = Some(folder));
        ctx.save_settings_now();
        // A scan of the previous folder still running or queued is of no use now.
        self.imp().folder.advance();
        // The watcher moves first, so changes reported for the old folder can't bring
        // its tracks back after the scan has dropped them.
        self.update_watcher();
        self.rescan(false);
    }

    /// Removes every track from the library, for when a scan found the music folder
    /// empty and the user confirms the music is gone. Playlists are kept.
    pub fn clear_tracks(&self) {
        self.send(Job::ClearTracks);
    }

    /// Starts or stops watching the music folder according to the settings.
    pub fn update_watcher(&self) {
        let Some(ctx) = self.context() else { return };
        let watch = ctx.settings().watch_music_folder;
        let options = self.scan_options(false).filter(|_| watch);
        self.send(Job::Watch(options));
    }

    fn scan_options(&self, force: bool) -> Option<ScanOptions> {
        let ctx = self.context()?;
        let root = ctx.settings().music_folder.clone()?;
        Some(ScanOptions {
            root,
            covers_dir: ctx.paths.covers_dir(),
            force,
        })
    }

    fn context(&self) -> Option<Rc<AppContext>> {
        self.imp().context.borrow().upgrade()
    }

    fn send(&self, job: Job) {
        let jobs = self.imp().jobs.borrow();
        let Some(jobs) = jobs.as_ref() else { return };
        if jobs.send(job).is_err() {
            log::error!("The library thread has stopped");
        }
    }

    fn receive_updates(&self, updates: async_channel::Receiver<Update>) {
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            while let Ok(update) = updates.recv().await {
                let Some(service) = weak.upgrade() else { break };
                let Some(ctx) = service.context() else { break };
                service.apply(&ctx, update);
            }
        });
    }

    fn apply(&self, ctx: &AppContext, update: Update) {
        match update {
            Update::Library {
                snapshot,
                playlists,
            } => {
                let track_count = snapshot.tracks.len();
                ctx.store.replace(snapshot);
                ctx.store.set_playlists(playlists);
                ctx.covers.library_changed();
                ctx.player
                    .broadcast(PlayerEvent::LibraryUpdated { track_count });
            }
            Update::ScanStarted => {
                self.set_progress(0, 0);
                self.set_scanning(true);
            }
            Update::ScanProgress(progress) => {
                self.set_progress(progress.processed, progress.total);
            }
            Update::ScanFinished(summary) => {
                self.set_scanning(false);
                self.set_folder_missing(false);
                // Full scans delete covers no track uses any more; their thumbnails go too.
                if summary.removed + summary.updated > 0 {
                    ctx.covers.remove_unused_thumbnails();
                }
                if let Some(message) = summary_message(&summary) {
                    ctx.toast(&message);
                }
            }
            Update::FolderUnavailable(folder) => {
                log::warn!("Music folder {} is unavailable", folder.display());
                self.set_scanning(false);
                self.set_folder_missing(true);
            }
            Update::Error(message) => {
                log::error!("{message}");
                ctx.toast(&message);
            }
        }
    }

    fn set_scanning(&self, scanning: bool) {
        if self.imp().scanning.replace(scanning) != scanning {
            self.notify_scanning();
        }
    }

    fn set_progress(&self, processed: usize, total: usize) {
        let imp = self.imp();
        let (processed, total) = (processed as u32, total as u32);
        if imp.scan_processed.replace(processed) != processed {
            self.notify_scan_processed();
        }
        if imp.scan_total.replace(total) != total {
            self.notify_scan_total();
        }
    }

    fn set_folder_missing(&self, missing: bool) {
        if self.imp().folder_missing.replace(missing) != missing {
            self.notify_folder_missing();
            self.follow_mounts(missing);
        }
    }

    /// While the music folder is missing, a drive being mounted may bring it back, so
    /// every new mount triggers a scan, which clears the banner once the folder is found.
    fn follow_mounts(&self, follow: bool) {
        let mounts = &self.imp().mounts;
        if let Some((monitor, handler)) = mounts.take() {
            monitor.disconnect(handler);
        }
        if !follow {
            return;
        }
        let monitor = gio::VolumeMonitor::get();
        let weak = self.downgrade();
        let handler = monitor.connect_mount_added(move |_, _| {
            if let Some(service) = weak.upgrade() {
                service.rescan(false);
            }
        });
        mounts.replace(Some((monitor, handler)));
    }
}

/// A short toast describing what a scan changed, or `None` if there is nothing to say.
fn summary_message(summary: &ScanSummary) -> Option<String> {
    let mut parts = Vec::new();
    if summary.added > 0 {
        parts.push(format!("{} added", song_count(summary.added)));
    }
    if summary.updated > 0 {
        parts.push(format!("{} updated", summary.updated));
    }
    if summary.removed > 0 {
        parts.push(format!("{} removed", summary.removed));
    }
    if summary.failed > 0 {
        parts.push(format!(
            "{} couldn’t be read",
            count_label(summary.failed, "file", "files")
        ));
    }
    (!parts.is_empty()).then(|| format!("Library updated: {}", parts.join(", ")))
}

/// Counts the user's changes of music folder, so the library thread can tell when a
/// scan has been overtaken by one.
#[derive(Clone, Default)]
pub(super) struct FolderGeneration(Arc<AtomicU64>);

impl FolderGeneration {
    fn current(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }

    fn advance(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

fn run_worker(
    db_path: &Path,
    jobs: &mpsc::Receiver<Job>,
    watcher_jobs: mpsc::Sender<Job>,
    folder: FolderGeneration,
    updates: async_channel::Sender<Update>,
) {
    let library = match Library::open(db_path) {
        Ok(library) => library,
        Err(err) => {
            let _ = updates.send_blocking(Update::Error(format!(
                "Couldn’t open the library database: {err}"
            )));
            return;
        }
    };
    let mut worker = Worker {
        library,
        updates,
        watcher_jobs,
        folder,
        library_outdated: false,
        options: None,
        watcher: None,
    };
    while let Ok(job) = jobs.recv() {
        match job {
            Job::Load => worker.send_library(),
            Job::Scan { options, folder } => worker.scan(options, folder),
            Job::Changed(paths) => worker.scan_paths(&paths),
            Job::Watch(options) => worker.watch(options),
            Job::ClearTracks => worker.clear_tracks(),
        }
    }
}

/// State owned by the library thread.
struct Worker {
    library: Library,
    updates: async_channel::Sender<Update>,
    /// Lets the watcher callback queue work for this thread.
    watcher_jobs: mpsc::Sender<Job>,
    folder: FolderGeneration,
    /// A cancelled scan may have saved changes the UI hasn't been sent.
    library_outdated: bool,
    /// Options for scanning paths reported by the watcher.
    options: Option<ScanOptions>,
    watcher: Option<LibraryWatcher>,
}

impl Worker {
    fn send(&self, update: Update) {
        // Fails only once the UI has gone away, when there's nobody left to tell.
        let _ = self.updates.send_blocking(update);
    }

    fn send_library(&self) {
        let loaded = self.library.tracks().and_then(|tracks| {
            let playlists = self.library.playlists()?;
            Ok((LibrarySnapshot::new(tracks), playlists))
        });
        match loaded {
            Ok((snapshot, playlists)) => self.send(Update::Library {
                snapshot,
                playlists,
            }),
            Err(err) => self.send(Update::Error(format!(
                "Couldn’t read the music library: {err}"
            ))),
        }
    }

    fn scan(&mut self, options: ScanOptions, folder: u64) {
        let generation = self.folder.clone();
        let outdated = || generation.current() != folder;
        if outdated() {
            return;
        }
        self.send(Update::ScanStarted);
        let reporter = ProgressReporter::new(self.updates.clone());
        let result = self.library.scan_cancellable(
            &options,
            &|progress| reporter.report(progress),
            &outdated,
        );
        self.finish_scan(result);
    }

    fn scan_paths(&mut self, paths: &[PathBuf]) {
        let Some(options) = self.options.clone() else {
            return;
        };
        self.send(Update::ScanStarted);
        let result = self.library.scan_paths(&options, paths);
        self.finish_scan(result);
    }

    fn finish_scan(&mut self, result: mp_library::Result<ScanSummary>) {
        let summary = match result {
            Ok(summary) => {
                self.resume_watching();
                summary
            }
            // The library refuses to scan an unavailable folder rather than empty itself.
            Err(LibraryError::FolderUnavailable(folder)) => {
                // An empty folder may be the mount point of an unplugged drive, which
                // the drive will cover when it is back.
                self.watcher = None;
                self.send(Update::FolderUnavailable(folder));
                return;
            }
            // The scan of the new folder is queued right behind; its result is sent.
            Err(LibraryError::Cancelled) => {
                self.library_outdated = true;
                return;
            }
            Err(err) => {
                self.send(Update::Error(format!(
                    "Scanning the music folder failed: {err}"
                )));
                ScanSummary::default()
            }
        };
        if summary.changed() || std::mem::take(&mut self.library_outdated) {
            self.send_library();
        }
        self.send(Update::ScanFinished(summary));
    }

    fn clear_tracks(&mut self) {
        match self.library.clear_tracks() {
            Ok(removed) => {
                log::info!("Removed {removed} tracks at the user’s request");
                self.send_library();
            }
            Err(err) => self.send(Update::Error(format!("Couldn’t clear the library: {err}"))),
        }
    }

    /// Watching stops while the music folder is unavailable, or never starts if it was
    /// missing; once a scan finds the folder again, it starts afresh.
    fn resume_watching(&mut self) {
        let watching = self
            .watcher
            .as_ref()
            .is_some_and(LibraryWatcher::is_watching);
        if !watching && self.options.is_some() {
            self.watch(self.options.clone());
        }
    }

    fn watch(&mut self, options: Option<ScanOptions>) {
        self.watcher = None;
        self.options = options.clone();
        let Some(options) = options else { return };
        if !options.root.is_dir() {
            // Already reported by the scan; there is nothing to watch until it's back.
            log::info!("Not watching {}: it doesn’t exist", options.root.display());
            return;
        }
        let jobs = self.watcher_jobs.clone();
        match LibraryWatcher::new(&options.root, move |paths| {
            let _ = jobs.send(Job::Changed(paths));
        }) {
            Ok(watcher) => self.watcher = Some(watcher),
            Err(err) => self.send(Update::Error(format!(
                "Can’t watch {} for changes: {err}",
                options.root.display()
            ))),
        }
    }
}

/// Forwards scan progress to the UI, throttled to [`PROGRESS_INTERVAL`].
struct ProgressReporter {
    updates: async_channel::Sender<Update>,
    last_sent: Mutex<Option<Instant>>,
}

impl ProgressReporter {
    fn new(updates: async_channel::Sender<Update>) -> Self {
        Self {
            updates,
            last_sent: Mutex::new(None),
        }
    }

    fn report(&self, progress: ScanProgress) {
        let finished = progress.processed >= progress.total;
        let mut last_sent = self
            .last_sent
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if !finished && last_sent.is_some_and(|time| time.elapsed() < PROGRESS_INTERVAL) {
            return;
        }
        *last_sent = Some(Instant::now());
        let _ = self.updates.try_send(Update::ScanProgress(progress));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worker(data_dir: &Path) -> (Worker, async_channel::Receiver<Update>) {
        let (updates, receiver) = async_channel::unbounded();
        let worker = Worker {
            library: Library::open(&data_dir.join("library.db")).unwrap(),
            updates,
            watcher_jobs: mpsc::channel().0,
            folder: FolderGeneration::default(),
            library_outdated: false,
            options: None,
            watcher: None,
        };
        (worker, receiver)
    }

    fn is_watching(worker: &Worker) -> bool {
        worker
            .watcher
            .as_ref()
            .is_some_and(LibraryWatcher::is_watching)
    }

    #[test]
    fn watching_starts_once_a_scan_finds_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let (mut worker, _updates) = worker(dir.path());
        let options = ScanOptions {
            root: dir.path().join("music"),
            covers_dir: dir.path().join("covers"),
            force: false,
        };
        worker.watch(Some(options.clone()));
        assert!(!is_watching(&worker), "the folder is missing");

        std::fs::create_dir(&options.root).unwrap();
        worker.scan(options.clone(), 0);
        assert!(is_watching(&worker));

        worker.watch(None);
        worker.scan(options, 0);
        assert!(worker.watcher.is_none(), "watching was turned off");
    }

    /// A scan of a folder the user has moved away from is skipped, or stopped, and
    /// the next scan sends the library even if it changes nothing itself.
    #[test]
    fn scans_of_a_previous_folder_are_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let (mut worker, updates) = worker(dir.path());
        let root = dir.path().join("music");
        std::fs::create_dir(&root).unwrap();
        let options = ScanOptions {
            root,
            covers_dir: dir.path().join("covers"),
            force: false,
        };
        worker.folder.advance();
        worker.scan(options.clone(), 0);
        assert!(updates.is_empty(), "the outdated scan didn't run");

        worker.library_outdated = true;
        worker.scan(options, 1);
        let sent: Vec<Update> = std::iter::from_fn(|| updates.try_recv().ok())
            .filter(|update| !matches!(update, Update::ScanProgress(_)))
            .collect();
        assert!(matches!(
            sent.as_slice(),
            [
                Update::ScanStarted,
                Update::Library { .. },
                Update::ScanFinished(_)
            ]
        ));
    }

    #[test]
    fn summary_messages() {
        assert_eq!(summary_message(&ScanSummary::default()), None);
        let summary = ScanSummary {
            added: 12,
            removed: 1,
            failed: 1,
            ..ScanSummary::default()
        };
        assert_eq!(
            summary_message(&summary).as_deref(),
            Some("Library updated: 12 songs added, 1 removed, 1 file couldn’t be read")
        );
    }
}
