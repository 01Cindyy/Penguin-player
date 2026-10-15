//! Album art at the size it is shown: decoded and scaled off the main thread, kept in
//! a memory-bounded LRU, and written to disk as PNG thumbnails so the next start is
//! instant.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fs;
use std::future::Future;
use std::hash::Hash;
use std::path::{Path, PathBuf};

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, gdk_pixbuf, gio, glib};
use mp_core::{AppPaths, CoverId};

/// Pixel data of decoded covers kept in memory.
const MEMORY_BUDGET_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CoverKey {
    cover: CoverId,
    size: u32,
}

type Waiter = async_channel::Sender<Option<gdk::Texture>>;

struct Inner {
    covers_dir: PathBuf,
    thumbnails_dir: PathBuf,
    memory: RefCell<Lru<CoverKey, gdk::Texture>>,
    /// Requests waiting for a decode that is already running.
    pending: RefCell<HashMap<CoverKey, Vec<Waiter>>>,
    /// Covers whose image could not be decoded, so they aren't retried on every bind.
    failed: RefCell<HashSet<CoverKey>>,
}

mod imp {
    use std::cell::OnceCell;
    use std::sync::OnceLock;

    use gtk::glib;
    use gtk::glib::subclass::Signal;
    use gtk::subclass::prelude::*;

    #[derive(Default)]
    pub struct CoverCache {
        pub(super) inner: OnceCell<super::Inner>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for CoverCache {
        const NAME: &'static str = "MpCoverCache";
        type Type = super::CoverCache;
    }

    impl ObjectImpl for CoverCache {
        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| vec![Signal::builder("covers-changed").build()])
        }
    }
}

glib::wrapper! {
    /// Loads album art. Cheap to clone; all clones share one cache.
    pub struct CoverCache(ObjectSubclass<imp::CoverCache>);
}

impl CoverCache {
    pub fn new(paths: &AppPaths) -> Self {
        let cache: Self = glib::Object::new();
        let inner = Inner {
            covers_dir: paths.covers_dir(),
            thumbnails_dir: paths.thumbnails_dir(),
            memory: RefCell::new(Lru::new(MEMORY_BUDGET_BYTES)),
            pending: RefCell::default(),
            failed: RefCell::default(),
        };
        assert!(
            cache.imp().inner.set(inner).is_ok(),
            "a new cache has no state yet"
        );
        cache
    }

    fn inner(&self) -> &Inner {
        self.imp()
            .inner
            .get()
            .expect("the state is set when the cache is created")
    }

    /// Tells covers that found no image to look again: after the library changed, the
    /// scanner may have extracted it (for example after the cache folder was deleted).
    pub fn library_changed(&self) {
        self.emit_by_name::<()>("covers-changed", &[]);
    }

    /// Called after [`Self::library_changed`].
    pub fn connect_covers_changed<F: Fn(&Self) + 'static>(&self, f: F) -> glib::SignalHandlerId {
        self.connect_closure(
            "covers-changed",
            false,
            glib::closure_local!(move |cache: &Self| f(cache)),
        )
    }

    /// Deletes, in the background, the thumbnails of covers the scanner has deleted
    /// because no track uses them any more.
    pub fn remove_unused_thumbnails(&self) {
        let inner = self.inner();
        let covers_dir = inner.covers_dir.clone();
        let thumbnails_dir = inner.thumbnails_dir.clone();
        drop(gio::spawn_blocking(move || {
            remove_orphaned_thumbnails(&covers_dir, &thumbnails_dir);
        }));
    }

    /// The full-size image extracted by the library scanner.
    pub fn source_path(&self, cover: &CoverId) -> PathBuf {
        self.inner().covers_dir.join(file_name(cover))
    }

    /// A cover already decoded at `size`, if there is one; never waits.
    pub fn cached(&self, cover: &CoverId, size: u32) -> Option<gdk::Texture> {
        let key = CoverKey {
            cover: cover.clone(),
            size,
        };
        self.inner().memory.borrow_mut().get(&key)
    }

    /// Loads `cover` with its shorter side scaled to `size` physical pixels (never
    /// enlarged). Concurrent requests for the same cover and size share one decode.
    /// Resolves to `None` if the image can't be read.
    pub fn load(&self, cover: &CoverId, size: u32) -> impl Future<Output = Option<gdk::Texture>> {
        let key = CoverKey {
            cover: cover.clone(),
            size,
        };
        let lookup = self.lookup(key);
        async move {
            match lookup {
                Lookup::Ready(texture) => texture,
                Lookup::Pending(receiver) => receiver.recv().await.ok().flatten(),
            }
        }
    }

    fn lookup(&self, key: CoverKey) -> Lookup {
        let inner = self.inner();
        if let Some(texture) = inner.memory.borrow_mut().get(&key) {
            return Lookup::Ready(Some(texture));
        }
        if inner.failed.borrow().contains(&key) {
            return Lookup::Ready(None);
        }
        let (sender, receiver) = async_channel::bounded(1);
        let first_request = {
            let mut pending = inner.pending.borrow_mut();
            let waiters = pending.entry(key.clone()).or_default();
            waiters.push(sender);
            waiters.len() == 1
        };
        if first_request {
            self.spawn_decode(key);
        }
        Lookup::Pending(receiver)
    }

    fn spawn_decode(&self, key: CoverKey) {
        let cache = self.clone();
        let source = self.source_path(&key.cover);
        let thumbnail = self.thumbnail_path(&key);
        let size = key.size;
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(move || load_texture(&source, &thumbnail, size)).await;
            let texture = match result {
                Ok(Ok(texture)) => Some(texture),
                // Not remembered: the next scan extracts the image again.
                Ok(Err(LoadError::Missing)) => None,
                Ok(Err(LoadError::Unreadable(err))) => {
                    log::debug!("Cover {} unavailable: {err}", key.cover.0);
                    cache.inner().failed.borrow_mut().insert(key.clone());
                    None
                }
                Err(_) => {
                    log::warn!("Decoding cover {} panicked", key.cover.0);
                    cache.inner().failed.borrow_mut().insert(key.clone());
                    None
                }
            };
            cache.finish(key, texture);
        });
    }

    fn finish(&self, key: CoverKey, texture: Option<gdk::Texture>) {
        let inner = self.inner();
        if let Some(texture) = &texture {
            let cost = texture_cost(texture);
            inner
                .memory
                .borrow_mut()
                .insert(key.clone(), texture.clone(), cost);
        }
        let waiters = inner.pending.borrow_mut().remove(&key).unwrap_or_default();
        for waiter in waiters {
            // A waiter whose widget went away has dropped its receiver; that's fine.
            let _ = waiter.try_send(texture.clone());
        }
    }

    fn thumbnail_path(&self, key: &CoverKey) -> PathBuf {
        let stem = Path::new(file_name(&key.cover)).file_stem().map_or_else(
            || key.cover.0.clone(),
            |stem| stem.to_string_lossy().into_owned(),
        );
        self.inner()
            .thumbnails_dir
            .join(key.size.to_string())
            .join(format!("{stem}.png"))
    }
}

enum Lookup {
    Ready(Option<gdk::Texture>),
    Pending(async_channel::Receiver<Option<gdk::Texture>>),
}

/// The cover's file name, ignoring any directory part so an id can't point elsewhere.
fn file_name(cover: &CoverId) -> &str {
    Path::new(&cover.0)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
}

/// Removes the `<size>/<cover>.png` thumbnails whose cover is no longer in
/// `covers_dir`. Leaves everything alone if the covers can't be listed.
fn remove_orphaned_thumbnails(covers_dir: &Path, thumbnails_dir: &Path) {
    let stem = |name: &OsStr| -> Option<String> {
        let name = name.to_str()?;
        Some(
            name.split_once('.')
                .map_or(name, |(stem, _)| stem)
                .to_owned(),
        )
    };
    let Ok(covers) = fs::read_dir(covers_dir) else {
        return;
    };
    let in_use: HashSet<String> = covers
        .filter_map(|entry| stem(&entry.ok()?.file_name()))
        .collect();
    let Ok(sizes) = fs::read_dir(thumbnails_dir) else {
        return;
    };
    for size_dir in sizes.filter_map(|entry| Some(entry.ok()?.path())) {
        let Ok(thumbnails) = fs::read_dir(&size_dir) else {
            continue;
        };
        for path in thumbnails.filter_map(|entry| Some(entry.ok()?.path())) {
            let is_png = path.extension().is_some_and(|extension| extension == "png");
            let orphaned = path
                .file_name()
                .and_then(stem)
                .is_some_and(|stem| !in_use.contains(&stem));
            if is_png
                && orphaned
                && let Err(err) = fs::remove_file(&path)
            {
                log::debug!("Could not remove thumbnail {}: {err}", path.display());
            }
        }
    }
}

fn texture_cost(texture: &gdk::Texture) -> usize {
    texture.width().max(0) as usize * texture.height().max(0) as usize * 4
}

/// Why a cover couldn't be loaded.
#[derive(Debug, PartialEq)]
enum LoadError {
    /// The scanner hasn't extracted the image (yet).
    Missing,
    Unreadable(String),
}

/// Runs on GIO's blocking pool: reuse the thumbnail if there is one, otherwise decode
/// the source scaled down and save a thumbnail for next time.
fn load_texture(source: &Path, thumbnail: &Path, size: u32) -> Result<gdk::Texture, LoadError> {
    if thumbnail.exists() {
        match gdk::Texture::from_filename(thumbnail) {
            Ok(texture) => return Ok(texture),
            Err(err) => log::debug!("Regenerating {}: {err}", thumbnail.display()),
        }
    }
    if !source.exists() {
        return Err(LoadError::Missing);
    }
    let pixbuf = decode_scaled(source, size).map_err(LoadError::Unreadable)?;
    let texture = texture_from_pixbuf(&pixbuf);
    if let Err(err) = save_thumbnail(&texture, thumbnail) {
        log::debug!("Could not save thumbnail {}: {err}", thumbnail.display());
    }
    Ok(texture)
}

fn decode_scaled(path: &Path, size: u32) -> Result<gdk_pixbuf::Pixbuf, String> {
    let (_, width, height) = gdk_pixbuf::Pixbuf::file_info(path)
        .ok_or_else(|| format!("{} is not a readable image", path.display()))?;
    let (width, height) = scaled_size(width, height, size);
    let pixbuf = gdk_pixbuf::Pixbuf::from_file_at_scale(path, width, height, false)
        .map_err(|err| err.to_string())?;
    Ok(pixbuf.apply_embedded_orientation().unwrap_or(pixbuf))
}

/// Dimensions that make the shorter side `size` pixels, never enlarging the image.
fn scaled_size(width: i32, height: i32, size: u32) -> (i32, i32) {
    let shorter = f64::from(width.min(height).max(1));
    let scale = (f64::from(size) / shorter).min(1.0);
    let scale_side = |side: i32| ((f64::from(side) * scale).round() as i32).max(1);
    (scale_side(width), scale_side(height))
}

fn texture_from_pixbuf(pixbuf: &gdk_pixbuf::Pixbuf) -> gdk::Texture {
    let format = if pixbuf.has_alpha() {
        gdk::MemoryFormat::R8g8b8a8
    } else {
        gdk::MemoryFormat::R8g8b8
    };
    gdk::MemoryTexture::new(
        pixbuf.width(),
        pixbuf.height(),
        format,
        &pixbuf.read_pixel_bytes(),
        pixbuf.rowstride() as usize,
    )
    .upcast()
}

/// Writes to a temporary file first so a crash never leaves a truncated thumbnail.
fn save_thumbnail(texture: &gdk::Texture, path: &Path) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|err| err.to_string())?;
    }
    let partial = path.with_extension("png.part");
    texture
        .save_to_png(&partial)
        .map_err(|err| err.to_string())?;
    std::fs::rename(&partial, path).map_err(|err| err.to_string())
}

/// A least-recently-used map bounded by the total cost of its values.
struct Lru<K, V> {
    entries: HashMap<K, LruEntry<V>>,
    budget: usize,
    used: usize,
    clock: u64,
}

struct LruEntry<V> {
    value: V,
    cost: usize,
    last_used: u64,
}

impl<K: Hash + Eq + Clone, V: Clone> Lru<K, V> {
    fn new(budget: usize) -> Self {
        Self {
            entries: HashMap::new(),
            budget,
            used: 0,
            clock: 0,
        }
    }

    fn get(&mut self, key: &K) -> Option<V> {
        self.clock += 1;
        let entry = self.entries.get_mut(key)?;
        entry.last_used = self.clock;
        Some(entry.value.clone())
    }

    fn insert(&mut self, key: K, value: V, cost: usize) {
        if let Some(old) = self.entries.remove(&key) {
            self.used -= old.cost;
        }
        while self.used + cost > self.budget && self.evict_oldest() {}
        self.clock += 1;
        self.used += cost;
        self.entries.insert(
            key,
            LruEntry {
                value,
                cost,
                last_used: self.clock,
            },
        );
    }

    fn evict_oldest(&mut self) -> bool {
        let oldest = self
            .entries
            .iter()
            .min_by_key(|(_, entry)| entry.last_used)
            .map(|(key, _)| key.clone());
        match oldest.and_then(|key| self.entries.remove(&key)) {
            Some(entry) => {
                self.used -= entry.cost;
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lru_evicts_least_recently_used() {
        let mut lru = Lru::new(10);
        lru.insert("a", 1, 4);
        lru.insert("b", 2, 4);
        assert_eq!(lru.get(&"a"), Some(1));
        lru.insert("c", 3, 4);
        assert_eq!(lru.get(&"b"), None);
        assert_eq!(lru.get(&"a"), Some(1));
        assert_eq!(lru.get(&"c"), Some(3));
        lru.insert("a", 5, 2);
        assert_eq!(lru.used, 6);
    }

    #[test]
    fn thumbnails_of_deleted_covers_are_removed() {
        let dir = tempfile::tempdir().unwrap();
        let (covers, thumbnails) = (dir.path().join("covers"), dir.path().join("thumbnails"));
        fs::create_dir_all(&covers).unwrap();
        fs::write(covers.join("00000000000000aa.jpg"), "").unwrap();
        for size in ["36", "200"] {
            fs::create_dir_all(thumbnails.join(size)).unwrap();
            for name in ["00000000000000aa.png", "00000000000000bb.png", "notes.txt"] {
                fs::write(thumbnails.join(size).join(name), "").unwrap();
            }
        }
        remove_orphaned_thumbnails(&covers, &thumbnails);
        for size in ["36", "200"] {
            let mut left: Vec<_> = fs::read_dir(thumbnails.join(size))
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect();
            left.sort();
            assert_eq!(left, ["00000000000000aa.png", "notes.txt"]);
        }

        // Without a covers folder to compare with, nothing is deleted.
        remove_orphaned_thumbnails(&dir.path().join("missing"), &thumbnails);
        assert!(thumbnails.join("36/00000000000000aa.png").exists());
    }

    #[test]
    fn scaling_fills_requested_size_without_enlarging() {
        assert_eq!(scaled_size(1000, 1000, 200), (200, 200));
        assert_eq!(scaled_size(1200, 800, 200), (300, 200));
        assert_eq!(scaled_size(100, 100, 200), (100, 100));
    }

    #[test]
    fn thumbnails_are_keyed_by_size_and_cover() {
        let cache = CoverCache::new(&AppPaths::from_root(Path::new("/tmp/mp")));
        let key = CoverKey {
            cover: CoverId("../escape/ab12.jpg".into()),
            size: 96,
        };
        assert_eq!(
            cache.thumbnail_path(&key),
            Path::new("/tmp/mp/cache/thumbnails/96/ab12.png")
        );
        assert_eq!(
            cache.source_path(&key.cover),
            Path::new("/tmp/mp/cache/covers/ab12.jpg")
        );
    }

    #[test]
    fn images_missing_from_the_cache_are_tried_again() {
        let dir = tempfile::tempdir().unwrap();
        let cache = CoverCache::new(&AppPaths::from_root(dir.path()));
        let cover = CoverId("ab12.png".into());
        let unreadable = CoverId("cd34.png".into());
        std::fs::create_dir_all(cache.source_path(&cover).parent().unwrap()).unwrap();
        std::fs::write(cache.source_path(&unreadable), "not an image").unwrap();

        // Decodes are spawned on the thread's default main context, so `load` is
        // called once this context is the default.
        let context = glib::MainContext::new();
        let load = |cover| context.block_on(async { cache.load(cover, 64).await });
        assert!(load(&cover).is_none());
        assert!(load(&unreadable).is_none());

        let pixbuf =
            gdk_pixbuf::Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, false, 8, 100, 100).unwrap();
        for id in [&cover, &unreadable] {
            pixbuf.savev(cache.source_path(id), "png", &[]).unwrap();
        }
        assert!(load(&cover).is_some());
        assert!(
            load(&unreadable).is_none(),
            "a broken image isn't decoded again"
        );
    }

    #[test]
    fn decodes_scales_and_writes_thumbnail() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("cover.png");
        let pixbuf =
            gdk_pixbuf::Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, false, 8, 400, 300).unwrap();
        pixbuf.fill(0x3366_99ff);
        pixbuf.savev(&source, "png", &[]).unwrap();

        let thumbnail = dir.path().join("thumbs/64/cover.png");
        let texture = load_texture(&source, &thumbnail, 64).unwrap();
        assert_eq!((texture.width(), texture.height()), (85, 64));
        assert!(thumbnail.exists());

        let reloaded = load_texture(&source, &thumbnail, 64).unwrap();
        assert_eq!((reloaded.width(), reloaded.height()), (85, 64));
    }
}
