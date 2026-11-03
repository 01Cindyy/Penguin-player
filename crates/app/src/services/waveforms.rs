//! The playing song's waveform, while the Waveform Seek Bar is on: read from the cache,
//! or measured in the background the first time the song plays. The cache file is named
//! after the song's path and starts with its size and modification time, so a changed
//! song is measured again.

use std::cell::{OnceCell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gtk::gio;
use gtk::glib;
use gtk::glib::prelude::*;
use gtk::glib::subclass::prelude::*;
use mp_audio::{WAVEFORM_POINTS, analyse_waveform};
use mp_core::util::stable_hash;
use mp_core::{AppPaths, Track};

/// A cache file's size and modification time fields, before the waveform itself.
const HEADER_BYTES: usize = 16;

mod imp {
    use std::sync::OnceLock;

    use gtk::glib::subclass::Signal;

    use super::*;

    #[derive(Default)]
    pub struct Waveforms {
        pub(super) dir: OnceCell<PathBuf>,
        /// The song whose waveform is wanted.
        pub(super) song: RefCell<Option<PathBuf>>,
        pub(super) levels: RefCell<Option<Rc<[u8]>>>,
        /// Stops the measuring of the song before.
        pub(super) cancel: RefCell<Option<Arc<AtomicBool>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Waveforms {
        const NAME: &'static str = "MpWaveforms";
        type Type = super::Waveforms;
    }

    impl ObjectImpl for Waveforms {
        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| vec![Signal::builder("changed").build()])
        }
    }
}

glib::wrapper! {
    /// Owned by [`crate::AppContext`]; reach it as `ctx.waveforms`.
    pub struct Waveforms(ObjectSubclass<imp::Waveforms>);
}

impl Waveforms {
    pub fn new(paths: &AppPaths) -> Self {
        let waveforms: Self = glib::Object::new();
        assert!(
            waveforms.imp().dir.set(paths.waveforms_dir()).is_ok(),
            "a new service has no folder yet"
        );
        waveforms
    }

    /// The waveform of the song last asked for, once it is known.
    pub fn levels(&self) -> Option<Rc<[u8]>> {
        self.imp().levels.borrow().clone()
    }

    /// Called when [`Self::levels`] changes.
    pub fn connect_changed<F: Fn(&Self) + 'static>(&self, f: F) -> glib::SignalHandlerId {
        self.connect_closure(
            "changed",
            false,
            glib::closure_local!(move |waveforms: &Self| f(waveforms)),
        )
    }

    /// Gets the waveform of `song`, dropping the previous one and stopping any measuring
    /// still under way. With `None`, there is nothing more to do until the next song.
    pub fn show(&self, song: Option<&Track>) {
        let imp = self.imp();
        let path = song.map(|song| song.path.clone());
        if *imp.song.borrow() == path {
            return;
        }
        imp.song.replace(path);
        if let Some(cancel) = imp.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        if imp.levels.take().is_some() {
            self.emit_by_name::<()>("changed", &[]);
        }
        let (Some(song), Some(dir)) = (song, imp.dir.get()) else {
            return;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        imp.cancel.replace(Some(cancel.clone()));
        let (dir, path, stamp) = (dir.clone(), song.path.clone(), Stamp::of(song));
        let waveforms = self.downgrade();
        glib::spawn_future_local(async move {
            let cancelled = cancel.clone();
            let levels = gio::spawn_blocking(move || load(&dir, &path, stamp, &cancelled)).await;
            let Some(waveforms) = waveforms.upgrade() else {
                return;
            };
            if !cancel.load(Ordering::Relaxed)
                && let Ok(Some(levels)) = levels
            {
                waveforms.imp().levels.replace(Some(levels.into()));
                waveforms.emit_by_name::<()>("changed", &[]);
            }
        });
    }
}

/// What tells that a song's file has changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    size: u64,
    modified: i64,
}

impl Stamp {
    fn of(song: &Track) -> Self {
        Self {
            size: song.file_size,
            modified: song.modified,
        }
    }

    fn to_bytes(self) -> [u8; HEADER_BYTES] {
        let mut bytes = [0; HEADER_BYTES];
        bytes[..8].copy_from_slice(&self.size.to_le_bytes());
        bytes[8..].copy_from_slice(&self.modified.to_le_bytes());
        bytes
    }
}

/// Runs on GIO's blocking pool: the song's waveform from the cache, or else measured and
/// saved for next time. `None` if it was cancelled or can't be measured.
fn load(dir: &Path, song: &Path, stamp: Stamp, cancelled: &AtomicBool) -> Option<Vec<u8>> {
    let file = dir.join(format!(
        "{:016x}",
        stable_hash(song.as_os_str().as_encoded_bytes())
    ));
    if let Some(levels) = read_cached(&file, stamp) {
        return Some(levels);
    }
    let levels = match analyse_waveform(song, cancelled) {
        Ok(levels) => levels?,
        Err(err) => {
            log::debug!("No waveform for {}: {err}", song.display());
            return None;
        }
    };
    let mut contents = stamp.to_bytes().to_vec();
    contents.extend_from_slice(&levels);
    if let Err(err) = std::fs::write(&file, contents) {
        log::debug!("Could not save the waveform of {}: {err}", song.display());
    }
    Some(levels)
}

/// The waveform in `file` if it is whole and of this version of the song.
fn read_cached(file: &Path, stamp: Stamp) -> Option<Vec<u8>> {
    let contents = std::fs::read(file).ok()?;
    let (header, levels) = contents.split_at_checked(HEADER_BYTES)?;
    (header == stamp.to_bytes() && levels.len() == WAVEFORM_POINTS).then(|| levels.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAMP: Stamp = Stamp {
        size: 12,
        modified: 34,
    };

    #[test]
    fn cached_waveforms_are_only_used_for_the_same_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("cached");
        let levels = vec![7; WAVEFORM_POINTS];
        let mut contents = STAMP.to_bytes().to_vec();
        contents.extend_from_slice(&levels);
        std::fs::write(&file, &contents).unwrap();

        assert_eq!(read_cached(&file, STAMP), Some(levels));
        let changed = Stamp {
            modified: 35,
            ..STAMP
        };
        assert_eq!(read_cached(&file, changed), None);

        std::fs::write(&file, &contents[..100]).unwrap();
        assert_eq!(read_cached(&file, STAMP), None);
        assert_eq!(read_cached(&dir.path().join("missing"), STAMP), None);
    }

    #[test]
    fn unreadable_songs_have_no_waveform() {
        let dir = tempfile::tempdir().unwrap();
        let song = dir.path().join("song.flac");
        std::fs::write(&song, "not audio").unwrap();
        let levels = load(dir.path(), &song, STAMP, &AtomicBool::new(false));
        assert_eq!(levels, None);
    }
}
