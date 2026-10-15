//! Cover art: stores an audio file's embedded picture, or failing that an image from its
//! folder, in the covers cache as `<content hash>.<ext>`, so identical images (e.g. the
//! same front cover embedded in every track of an album) are written only once.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use mp_core::CoverId;
use mp_core::util::stable_hash;

/// Folder image names, most preferred first. Matched case-insensitively.
const FOLDER_IMAGE_NAMES: &[&str] = &["cover", "folder", "front", "album"];
const FOLDER_IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp"];

/// Folder images bigger than this are skipped. Real covers are a few MB at most, and
/// reading a huge file into memory could exhaust it. Matches the limit for embedded
/// pictures in `tags`.
const MAX_FOLDER_IMAGE_BYTES: u64 = 256 * 1024 * 1024;

/// Every extension [`image_extension`] gives, i.e. that the cache's images can have.
const IMAGE_EXTENSIONS: &[&str] = &["jpg", "png", "webp", "gif", "tiff", "bmp"];

/// How old a temporary file must be before cleaning up removes it. Writing one image
/// never takes this long, so an older one was left behind by a write that never finished.
const STALE_TEMPORARY_FILE_AGE: Duration = Duration::from_secs(60 * 60);

/// Whether `path` is named like a folder image (`cover.jpg`, `Folder.PNG`, ...).
pub(crate) fn is_folder_image(path: &Path) -> bool {
    folder_image_rank(path).is_some()
}

/// The covers cache directory during one scan. Shared by the scan's worker threads.
pub(crate) struct CoverStore {
    dir: PathBuf,
    /// File names present in `dir`, so existence checks don't need to touch the disk.
    stored: Mutex<HashSet<String>>,
    /// The folder image of each directory, looked up at most once per scan.
    folder_covers: Mutex<HashMap<PathBuf, Arc<OnceLock<Option<CoverId>>>>>,
}

impl CoverStore {
    /// Creates the cache directory if needed and lists what is already in it.
    pub fn open(dir: &Path) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let stored = fs::read_dir(dir)?
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .collect();
        Ok(Self {
            dir: dir.to_path_buf(),
            stored: Mutex::new(stored),
            folder_covers: Mutex::default(),
        })
    }

    /// Whether the image file of `cover` is in the cache.
    pub fn contains(&self, cover: &CoverId) -> bool {
        lock(&self.stored).contains(&cover.0)
    }

    /// The cover from the first usable image file in `dir` with a folder image name.
    pub fn folder_cover(&self, dir: &Path) -> Option<CoverId> {
        let cell = Arc::clone(
            lock(&self.folder_covers)
                .entry(dir.to_path_buf())
                .or_default(),
        );
        // Threads asking about the same folder wait for the first one instead of repeating it.
        cell.get_or_init(|| self.find_folder_cover(dir)).clone()
    }

    /// Writes an image to the cache unless an identical one is already there. Returns
    /// `None` if the data isn't a recognised image format or can't be written.
    pub fn store(&self, data: &[u8]) -> Option<CoverId> {
        let extension = image_extension(data)?;
        let name = format!("{:016x}.{extension}", stable_hash(data));
        if lock(&self.stored).contains(&name) {
            return Some(CoverId(name));
        }
        match write_atomically(&self.dir.join(&name), data) {
            Ok(()) => {
                lock(&self.stored).insert(name.clone());
                Some(CoverId(name))
            }
            Err(err) => {
                log::warn!("Could not write cover image {name}: {err}");
                None
            }
        }
    }

    /// Deletes the cached images that no track uses any more (after a cover changed or
    /// an album was removed) and temporary files left behind by an interrupted write.
    /// Only files named the way this cache names them are touched.
    pub fn remove_unused(&self, in_use: &HashSet<String>) {
        lock(&self.stored).retain(|name| {
            let unused = match cache_file_kind(name) {
                Some(CacheFile::Image) => !in_use.contains(name),
                // A recent one may belong to a write still in progress elsewhere.
                Some(CacheFile::Temporary) => self.is_stale(name),
                None => false,
            };
            !unused || !self.remove(name)
        });
    }

    fn is_stale(&self, name: &str) -> bool {
        fs::metadata(self.dir.join(name))
            .and_then(|metadata| metadata.modified())
            .is_ok_and(|modified| {
                modified
                    .elapsed()
                    .is_ok_and(|age| age > STALE_TEMPORARY_FILE_AGE)
            })
    }

    /// Deletes a file from the cache, returning whether it is gone.
    fn remove(&self, name: &str) -> bool {
        match fs::remove_file(self.dir.join(name)) {
            Ok(()) => true,
            Err(err) => {
                log::debug!("Could not remove unused cover {name}: {err}");
                err.kind() == io::ErrorKind::NotFound
            }
        }
    }

    fn find_folder_cover(&self, dir: &Path) -> Option<CoverId> {
        folder_image_candidates(dir)
            .into_iter()
            .find_map(|path| match fs::read(&path) {
                Ok(data) => self.store(&data),
                Err(err) => {
                    log::debug!("Could not read folder image {}: {err}", path.display());
                    None
                }
            })
    }
}

/// How preferred a folder image name is (lower is better), or `None` if `path` isn't
/// named like one.
fn folder_image_rank(path: &Path) -> Option<(usize, usize)> {
    let stem = path.file_stem()?.to_str()?;
    let extension = path.extension()?.to_str()?;
    let name_rank = FOLDER_IMAGE_NAMES
        .iter()
        .position(|name| name.eq_ignore_ascii_case(stem))?;
    let extension_rank = FOLDER_IMAGE_EXTENSIONS
        .iter()
        .position(|ext| ext.eq_ignore_ascii_case(extension))?;
    Some((name_rank, extension_rank))
}

/// Image files in `dir` with a folder image name, most preferred first. Only regular
/// files (or symlinks to them) of a sane size count: reading a FIFO would block the scan
/// forever, and a huge file could exhaust memory.
fn folder_image_candidates(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut candidates: Vec<((usize, usize), PathBuf)> = entries
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            let rank = folder_image_rank(&path)?;
            let metadata = fs::metadata(&path).ok()?;
            let usable = metadata.is_file() && metadata.len() <= MAX_FOLDER_IMAGE_BYTES;
            usable.then_some((rank, path))
        })
        .collect();
    candidates.sort();
    candidates.into_iter().map(|(_, path)| path).collect()
}

/// File extension for image data, recognised by its magic bytes. Declared MIME types
/// are ignored: taggers often get them wrong, and data that isn't a real image is
/// better skipped (falling back to a folder image) than cached.
fn image_extension(data: &[u8]) -> Option<&'static str> {
    let is_webp = data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WEBP".as_slice());
    let extension = if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "jpg"
    } else if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        "png"
    } else if is_webp {
        "webp"
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        "gif"
    } else if data.starts_with(b"II*\0") || data.starts_with(b"MM\0*") {
        "tiff"
    } else if data.starts_with(b"BM") {
        "bmp"
    } else {
        return None;
    };
    Some(extension)
}

/// The kinds of file this cache writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CacheFile {
    /// `<16 hex digits>.<image extension>`.
    Image,
    /// The name an image has while it is being written (see [`write_atomically`]).
    Temporary,
}

/// What kind of cache file `name` is, or `None` if this cache didn't name it.
fn cache_file_kind(name: &str) -> Option<CacheFile> {
    let (hash, rest) = name.split_once('.')?;
    let is_hash = hash.len() == 16
        && hash
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'));
    if !is_hash {
        None
    } else if IMAGE_EXTENSIONS.contains(&rest) {
        Some(CacheFile::Image)
    } else if rest.ends_with(".tmp") {
        Some(CacheFile::Temporary)
    } else {
        None
    }
}

/// Writes through a temporary file and a rename, so no reader (another scan thread, or
/// the UI loading covers) ever sees a half-written image.
fn write_atomically(path: &Path, data: &[u8]) -> io::Result<()> {
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let unique = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    let temp = path.with_extension(format!("{}-{unique}.tmp", std::process::id()));
    fs::write(&temp, data)?;
    fs::rename(&temp, path).inspect_err(|_| {
        let _ = fs::remove_file(&temp);
    })
}

/// Locks a mutex, ignoring poisoning: the guarded caches stay valid even if a thread
/// panicked while holding the lock.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_image_types() {
        let images: [(&[u8], &str); 6] = [
            (&[0xFF, 0xD8, 0xFF, 0xE0], "jpg"),
            (b"\x89PNG\r\n\x1a\n....", "png"),
            (b"RIFF\0\0\0\0WEBPVP8 ", "webp"),
            (b"GIF89a..", "gif"),
            (b"II*\0....", "tiff"),
            (b"BM......", "bmp"),
        ];
        for (data, extension) in images {
            assert_eq!(image_extension(data), Some(extension));
            assert!(IMAGE_EXTENSIONS.contains(&extension));
        }
        assert_eq!(image_extension(b"RIFF\0\0\0\0WAVEfmt "), None);
        assert_eq!(image_extension(b"not an image"), None);
        assert_eq!(image_extension(b""), None);
    }

    #[test]
    fn recognises_only_its_own_file_names() {
        for extension in IMAGE_EXTENSIONS {
            let name = format!("0123456789abcdef.{extension}");
            assert_eq!(cache_file_kind(&name), Some(CacheFile::Image));
        }
        assert_eq!(
            cache_file_kind("0123456789abcdef.1234-5.tmp"),
            Some(CacheFile::Temporary)
        );
        for name in [
            "cover.jpg",
            "0123456789abcdef",
            "0123456789abcdef.txt",
            "0123456789abcde.jpg",
            "0123456789abcdeg.jpg",
            "0123456789ABCDEF.jpg",
            "notes.tmp",
        ] {
            assert_eq!(cache_file_kind(name), None, "{name}");
        }
    }

    #[test]
    fn prefers_cover_over_folder_images() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "album.png",
            "Folder.JPG",
            "COVER.webp",
            "cover.txt",
            "back.jpg",
        ] {
            fs::write(dir.path().join(name), b"x").unwrap();
        }
        let names: Vec<_> = folder_image_candidates(dir.path())
            .into_iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["COVER.webp", "Folder.JPG", "album.png"]);
        assert!(is_folder_image(Path::new("/music/A/Cover.Jpeg")));
        assert!(!is_folder_image(Path::new("/music/A/back.jpg")));
    }

    #[test]
    fn only_regular_files_of_a_sane_size_are_folder_images() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("cover.jpg")).unwrap();
        // Sparse, so it takes no real space.
        fs::File::create(dir.path().join("front.jpg"))
            .unwrap()
            .set_len(MAX_FOLDER_IMAGE_BYTES + 1)
            .unwrap();
        fs::write(dir.path().join("folder.png"), b"x").unwrap();
        assert_eq!(
            folder_image_candidates(dir.path()),
            [dir.path().join("folder.png")]
        );
    }
}
