//! Album art at the size it is shown: decoded and scaled off the main thread, kept in
//! a memory-bounded LRU, and written to disk as thumbnails so the next start is
//! instant.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fs;
use std::future::Future;
use std::hash::Hash;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, gdk_pixbuf, gio, glib};
use mp_core::util::stable_hash;
use mp_core::{AppPaths, CoverId, PlaylistId};

/// Pixel data of decoded covers kept in memory.
const MEMORY_BUDGET_BYTES: usize = 32 * 1024 * 1024;
/// How long the window stays in the background before decoded covers are let go.
const BACKGROUND_RELEASE_DELAY: Duration = Duration::from_secs(5 * 60);
/// Thumbnails of covers without transparency are JPEG, which for photos and detailed
/// art takes about a fifth of the space of PNG.
const THUMBNAIL_JPEG_QUALITY: &str = "90";
/// Starts the file name of every image chosen for a playlist, which tells them from
/// covers.
const PLAYLIST_IMAGE_PREFIX: &str = "playlist-";
/// The side of the square a playlist image is saved as, at most: page headers show them
/// up to 232 pixels wide, or twice that on high-resolution screens.
const PLAYLIST_IMAGE_SIZE: u32 = 512;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CoverKey {
    cover: CoverId,
    size: u32,
}

type Waiter = async_channel::Sender<Option<gdk::Texture>>;

struct Inner {
    covers_dir: PathBuf,
    opened_covers_dir: PathBuf,
    /// Covers of songs from outside the library, which are in `opened_covers_dir`.
    opened: RefCell<HashSet<CoverId>>,
    playlist_images_dir: PathBuf,
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
            opened_covers_dir: paths.opened_covers_dir(),
            opened: RefCell::default(),
            playlist_images_dir: paths.playlist_images_dir(),
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

    /// Lets go of decoded covers once `window` has been in the background for
    /// [`BACKGROUND_RELEASE_DELAY`]. Covers on screen stay, as their widgets hold them.
    pub fn release_in_background(&self, window: &impl IsA<gtk::Window>) {
        let timer: Rc<RefCell<Option<glib::SourceId>>> = Rc::default();
        window.connect_is_active_notify(glib::clone!(
            #[weak(rename_to = cache)]
            self,
            move |window| {
                if let Some(source) = timer.take() {
                    source.remove();
                }
                if window.is_active() {
                    return;
                }
                let (cache, fired) = (cache.clone(), timer.clone());
                let source = glib::timeout_add_local_once(BACKGROUND_RELEASE_DELAY, move || {
                    fired.take();
                    cache.inner().memory.borrow_mut().clear();
                    crate::memory::release_soon();
                });
                timer.replace(Some(source));
            }
        ));
    }

    /// Notes that `cover` belongs to a song from outside the library, and so is in
    /// [`AppPaths::opened_covers_dir`].
    pub fn add_opened(&self, cover: &CoverId) {
        self.inner().opened.borrow_mut().insert(cover.clone());
    }

    /// The full-size image extracted by the library scanner, or for a song from outside
    /// the library, when it was opened, or the image chosen for a playlist.
    pub fn source_path(&self, cover: &CoverId) -> PathBuf {
        let inner = self.inner();
        let dir = if file_name(cover).starts_with(PLAYLIST_IMAGE_PREFIX) {
            &inner.playlist_images_dir
        } else if inner.opened.borrow().contains(cover) {
            &inner.opened_covers_dir
        } else {
            &inner.covers_dir
        };
        dir.join(file_name(cover))
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

    /// Where the thumbnail of `key` is, without its extension (see [`save_thumbnail`]).
    fn thumbnail_path(&self, key: &CoverKey) -> PathBuf {
        let stem = Path::new(file_name(&key.cover)).file_stem().map_or_else(
            || key.cover.0.clone(),
            |stem| stem.to_string_lossy().into_owned(),
        );
        self.inner()
            .thumbnails_dir
            .join(key.size.to_string())
            .join(stem)
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

/// Copies the image at `source` to `dir` for playlist `id`, as a square from its middle,
/// and returns its id. Each version gets its own name, as thumbnails go by the name.
/// Slow, so call it off the main thread.
pub fn import_playlist_image(source: &Path, id: PlaylistId, dir: &Path) -> Result<CoverId, String> {
    let pixbuf = decode_scaled(source, PLAYLIST_IMAGE_SIZE)?;
    let side = pixbuf.width().min(pixbuf.height());
    let square = pixbuf.new_subpixbuf(
        (pixbuf.width() - side) / 2,
        (pixbuf.height() - side) / 2,
        side,
        side,
    );
    fs::create_dir_all(dir).map_err(|err| err.to_string())?;
    let partial = dir.join(format!("{PLAYLIST_IMAGE_PREFIX}{}.part", id.0));
    square
        .savev(&partial, "png", &[])
        .map_err(|err| err.to_string())?;
    let contents = fs::read(&partial).map_err(|err| err.to_string())?;
    let name = format!(
        "{PLAYLIST_IMAGE_PREFIX}{}-{:016x}.png",
        id.0,
        stable_hash(&contents)
    );
    fs::rename(&partial, dir.join(&name)).map_err(|err| err.to_string())?;
    Ok(CoverId(name))
}

/// Deletes an image chosen for a playlist, which is no longer used.
pub fn remove_playlist_image(dir: &Path, image: &CoverId) {
    let path = dir.join(file_name(image));
    if let Err(err) = fs::remove_file(&path) {
        log::debug!("Could not remove {}: {err}", path.display());
    }
}

/// Removes the `<size>/<cover>.jpg` and `.png` thumbnails whose cover is no longer in
/// `covers_dir`, and leaves those of images chosen for playlists, which are elsewhere.
/// Leaves everything alone if the covers can't be listed.
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
            let is_thumbnail = path
                .extension()
                .is_some_and(|extension| extension == "jpg" || extension == "png");
            let orphaned = path.file_name().and_then(stem).is_some_and(|stem| {
                !in_use.contains(&stem) && !stem.starts_with(PLAYLIST_IMAGE_PREFIX)
            });
            if is_thumbnail
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
    // PNG for covers with transparency, and for every thumbnail older versions saved.
    for extension in ["jpg", "png"] {
        let saved = thumbnail.with_added_extension(extension);
        if saved.exists() {
            match gdk::Texture::from_filename(&saved) {
                Ok(texture) => return Ok(texture),
                Err(err) => log::debug!("Regenerating {}: {err}", saved.display()),
            }
        }
    }
    if !source.exists() {
        return Err(LoadError::Missing);
    }
    let pixbuf = decode_scaled(source, size).map_err(LoadError::Unreadable)?;
    if let Err(err) = save_thumbnail(&pixbuf, thumbnail) {
        log::debug!("Could not save thumbnail {}: {err}", thumbnail.display());
    }
    Ok(texture_from_pixbuf(&pixbuf))
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

/// Saves `<path>.jpg`, or `<path>.png` for an image with transparency, which JPEG
/// can't hold. Writes to a temporary file first so a crash never leaves a truncated
/// thumbnail.
fn save_thumbnail(pixbuf: &gdk_pixbuf::Pixbuf, path: &Path) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|err| err.to_string())?;
    }
    let (extension, format, options): (_, _, &[_]) = if pixbuf.has_alpha() {
        ("png", "png", &[])
    } else {
        ("jpg", "jpeg", &[("quality", THUMBNAIL_JPEG_QUALITY)])
    };
    let path = path.with_added_extension(extension);
    let partial = path.with_added_extension("part");
    pixbuf
        .savev(&partial, format, options)
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

    /// Drops every entry, and the room the map took for them.
    fn clear(&mut self) {
        self.entries = HashMap::new();
        self.used = 0;
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
            for name in [
                "00000000000000aa.jpg",
                "00000000000000aa.png",
                "00000000000000bb.jpg",
                "00000000000000bb.png",
                "playlist-1-00000000000000cc.png",
                "notes.txt",
            ] {
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
            assert_eq!(
                left,
                [
                    "00000000000000aa.jpg",
                    "00000000000000aa.png",
                    "notes.txt",
                    "playlist-1-00000000000000cc.png"
                ]
            );
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
            Path::new("/tmp/mp/cache/thumbnails/96/ab12")
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
    fn playlist_images_are_saved_as_squares_from_the_middle() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("photo.png");
        let pixbuf =
            gdk_pixbuf::Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, false, 8, 1000, 800).unwrap();
        pixbuf.fill(0x3366_99ff);
        pixbuf.savev(&source, "png", &[]).unwrap();

        let images = dir.path().join("playlist-images");
        let image = import_playlist_image(&source, PlaylistId(7), &images).unwrap();
        assert!(image.0.starts_with("playlist-7-"));
        let saved = images.join(&image.0);
        let (_, width, height) = gdk_pixbuf::Pixbuf::file_info(&saved).unwrap();
        assert_eq!((width, height), (512, 512));

        let cache = CoverCache::new(&AppPaths::from_root(dir.path()));
        assert_eq!(
            cache.source_path(&image),
            dir.path().join("data/playlist-images").join(&image.0)
        );

        remove_playlist_image(&images, &image);
        assert!(!saved.exists());
    }

    #[test]
    fn files_that_are_not_images_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("notes.png");
        fs::write(&source, "not an image").unwrap();
        let images = dir.path().join("playlist-images");
        assert!(import_playlist_image(&source, PlaylistId(7), &images).is_err());
    }

    #[test]
    fn decodes_scales_and_writes_thumbnail() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("cover.png");
        let pixbuf =
            gdk_pixbuf::Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, false, 8, 400, 300).unwrap();
        pixbuf.fill(0x3366_99ff);
        pixbuf.savev(&source, "png", &[]).unwrap();

        let thumbnail = dir.path().join("thumbs/64/cover");
        let texture = load_texture(&source, &thumbnail, 64).unwrap();
        assert_eq!((texture.width(), texture.height()), (85, 64));
        assert!(dir.path().join("thumbs/64/cover.jpg").exists());

        let reloaded = load_texture(&source, &thumbnail, 64).unwrap();
        assert_eq!((reloaded.width(), reloaded.height()), (85, 64));
    }

    #[test]
    fn thumbnails_keep_transparency_and_older_ones_are_used() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("cover.png");
        let pixbuf =
            gdk_pixbuf::Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, true, 8, 100, 100).unwrap();
        pixbuf.fill(0x3366_9980);
        pixbuf.savev(&source, "png", &[]).unwrap();

        let thumbnail = dir.path().join("thumbs/64/cover");
        let texture = load_texture(&source, &thumbnail, 64).unwrap();
        assert_eq!(texture.format(), gdk::MemoryFormat::R8g8b8a8);
        assert!(dir.path().join("thumbs/64/cover.png").exists());
        assert!(!dir.path().join("thumbs/64/cover.jpg").exists());

        // Older versions saved every thumbnail as PNG.
        let old = dir.path().join("thumbs/64/old");
        let opaque =
            gdk_pixbuf::Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, false, 8, 64, 64).unwrap();
        opaque
            .savev(old.with_added_extension("png"), "png", &[])
            .unwrap();
        let reloaded = load_texture(&dir.path().join("missing.png"), &old, 64).unwrap();
        assert_eq!((reloaded.width(), reloaded.height()), (64, 64));
    }
}
