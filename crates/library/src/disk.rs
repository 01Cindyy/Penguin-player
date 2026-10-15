//! Finding the audio files on disk, with what a scan compares against the database:
//! their size, modification time and the folder images (cover.jpg etc.) next to them.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs::{self, Metadata};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use mp_core::files::is_hidden_name;
use mp_core::format::is_supported_extension;
use mp_core::util::stable_hash;
use walkdir::WalkDir;

use crate::covers;

/// Whether a file has an audio extension the library picks up.
pub(crate) fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(is_supported_extension)
}

/// Whether `path` is inside a hidden folder or is a hidden file, looking only at the
/// part below `root` (the music folder itself may be hidden).
pub(crate) fn is_hidden_below(root: &Path, path: &Path) -> bool {
    path.strip_prefix(root).is_ok_and(|relative| {
        relative
            .components()
            .any(|component| is_hidden_name(component.as_os_str()))
    })
}

/// A supported audio file found on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiskFile {
    pub path: PathBuf,
    pub size: u64,
    /// Modification time in whole seconds since the Unix epoch, as in `Track::modified`.
    pub modified: i64,
    /// The sub-second part of the modification time. Without it, a file rewritten with
    /// the same size within the same second as the last scan would look unchanged.
    pub modified_nanos: u32,
    /// Fingerprint of the folder images next to the file (see [`folder_fingerprints`]).
    pub folder_images: i64,
}

impl DiskFile {
    fn new(path: PathBuf, metadata: &Metadata) -> Self {
        let (modified, modified_nanos) = unix_time(metadata.modified());
        Self {
            path,
            size: metadata.len(),
            modified,
            modified_nanos,
            folder_images: 0,
        }
    }
}

/// What a scan found on disk.
#[derive(Default)]
pub(crate) struct DiskState {
    files: Vec<DiskFile>,
    /// Folder images next to the files, each with its own fingerprint.
    folder_images: Vec<(PathBuf, i64)>,
    /// Paths that exist but couldn't be read. Tracks at or under them are kept rather
    /// than treated as deleted, so a permissions problem or a symlink to an unmounted
    /// drive doesn't remove them.
    unreadable: Vec<PathBuf>,
}

impl DiskState {
    /// Adds a file, or everything inside a folder. Missing paths add nothing.
    pub fn add_path(&mut self, path: &Path) {
        match fs::metadata(path) {
            Ok(metadata) if metadata.is_dir() => self.walk(path, &|| false),
            Ok(metadata) if metadata.is_file() && is_audio_file(path) => {
                self.files
                    .push(DiskFile::new(path.to_path_buf(), &metadata));
                if let Some(dir) = path.parent() {
                    self.add_folder_images_in(dir);
                }
            }
            Ok(_) => {}
            Err(err) if is_missing(&err) && !is_behind_dangling_symlink(path) => {}
            Err(err) => {
                log::warn!("Could not read {}: {err}", path.display());
                self.unreadable.push(path.to_path_buf());
            }
        }
    }

    /// Collects the audio files and folder images inside `dir`, skipping hidden files and
    /// folders. Symlinks are followed; walkdir detects loops and reports them as errors.
    /// Stops early once `cancelled`, leaving the state incomplete.
    pub fn walk(&mut self, dir: &Path, cancelled: &dyn Fn() -> bool) {
        let entries = WalkDir::new(dir)
            .follow_links(true)
            .into_iter()
            .filter_entry(|entry| entry.depth() == 0 || !is_hidden_name(entry.file_name()));
        for entry in entries {
            if cancelled() {
                return;
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(err) => {
                    self.note_walk_error(&err);
                    continue;
                }
            };
            if !entry.file_type().is_file() {
                continue;
            }
            let is_audio = is_audio_file(entry.path());
            if !is_audio && !covers::is_folder_image(entry.path()) {
                continue;
            }
            // Fails if the file vanished during the walk, which makes it simply gone.
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if is_audio {
                self.files.push(DiskFile::new(entry.into_path(), &metadata));
            } else {
                self.add_folder_image(entry.into_path(), &metadata);
            }
        }
    }

    /// Whether no audio files were found and nothing failed to be read.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty() && self.unreadable.is_empty()
    }

    /// The audio files found, each once, with their folder image fingerprints filled in.
    /// Overlapping scan paths can find the same file twice.
    pub fn take_files(&mut self) -> Vec<DiskFile> {
        let mut files = std::mem::take(&mut self.files);
        files.sort_unstable_by(|a, b| a.path.cmp(&b.path));
        files.dedup_by(|a, b| a.path == b.path);
        let fingerprints = folder_fingerprints(std::mem::take(&mut self.folder_images));
        for file in &mut files {
            let dir = file.path.parent().unwrap_or(Path::new(""));
            file.folder_images = fingerprints.get(dir).copied().unwrap_or(0);
        }
        files
    }

    /// Whether `path` is at or under a path that couldn't be read.
    pub fn is_unreadable(&self, path: &Path) -> bool {
        self.unreadable.iter().any(|dir| path.starts_with(dir))
    }

    fn add_folder_images_in(&mut self, dir: &Path) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for path in entries.filter_map(|entry| Some(entry.ok()?.path())) {
            if !covers::is_folder_image(&path) {
                continue;
            }
            if let Some(metadata) = fs::metadata(&path).ok().filter(Metadata::is_file) {
                self.add_folder_image(path, &metadata);
            }
        }
    }

    fn add_folder_image(&mut self, path: PathBuf, metadata: &Metadata) {
        let name = path.file_name().unwrap_or_default();
        let fingerprint = image_fingerprint(name, metadata.len(), unix_time(metadata.modified()));
        self.folder_images.push((path, fingerprint));
    }

    fn note_walk_error(&mut self, err: &walkdir::Error) {
        let Some(path) = err.path() else { return };
        // A symlink loop leads back into folders that are walked anyway.
        if err.loop_ancestor().is_some() {
            log::debug!("Skipping symlink loop at {}", path.display());
            return;
        }
        log::warn!("Could not read {}: {err}", path.display());
        self.unreadable.push(path.to_path_buf());
    }
}

/// A path that doesn't exist, or whose parent turned out not to be a folder.
fn is_missing(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
    )
}

/// Whether `path`, or a folder it is in, is a symlink to something that doesn't exist
/// (any more, or right now: it may point at an unmounted drive).
fn is_behind_dangling_symlink(path: &Path) -> bool {
    path.ancestors().any(|ancestor| {
        fs::symlink_metadata(ancestor).is_ok_and(|metadata| metadata.is_symlink())
            && fs::metadata(ancestor).is_err()
    })
}

/// One folder image's fingerprint, from its name, size and modification time.
fn image_fingerprint(name: &OsStr, size: u64, (seconds, nanos): (i64, u32)) -> i64 {
    let mut bytes = name.as_encoded_bytes().to_vec();
    bytes.extend_from_slice(&size.to_le_bytes());
    bytes.extend_from_slice(&seconds.to_le_bytes());
    bytes.extend_from_slice(&nanos.to_le_bytes());
    // Reinterpreted as signed because SQLite stores signed 64-bit integers.
    stable_hash(&bytes) as i64
}

/// Combines the fingerprints of each folder's images into one per folder, which changes
/// whenever a folder image is added, removed, renamed or rewritten. Folders without
/// images are left out (their fingerprint is 0).
fn folder_fingerprints(mut images: Vec<(PathBuf, i64)>) -> HashMap<PathBuf, i64> {
    images.sort_unstable();
    images.dedup_by(|a, b| a.0 == b.0);
    let mut fingerprints: HashMap<PathBuf, i64> = HashMap::new();
    for (path, fingerprint) in images {
        let dir = path.parent().unwrap_or(Path::new("")).to_path_buf();
        let combined = fingerprints.entry(dir).or_default();
        *combined = combined.wrapping_add(fingerprint);
    }
    fingerprints
}

/// Seconds since the Unix epoch (negative before it) and the nanoseconds within that
/// second, or zero if the platform can't tell.
fn unix_time(time: io::Result<SystemTime>) -> (i64, u32) {
    let Ok(time) = time else { return (0, 0) };
    match time.duration_since(UNIX_EPOCH) {
        Ok(after) => (
            i64::try_from(after.as_secs()).unwrap_or(i64::MAX),
            after.subsec_nanos(),
        ),
        Err(before) => {
            let before = before.duration();
            let seconds = i64::try_from(before.as_secs()).map_or(i64::MIN, |secs| -secs);
            (seconds, before.subsec_nanos())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn times_keep_their_nanoseconds() {
        let time = UNIX_EPOCH + Duration::new(1_700_000_000, 123_456_789);
        assert_eq!(unix_time(Ok(time)), (1_700_000_000, 123_456_789));
        let before = UNIX_EPOCH - Duration::new(5, 0);
        assert_eq!(unix_time(Ok(before)), (-5, 0));
        assert_eq!(unix_time(Err(io::ErrorKind::Unsupported.into())), (0, 0));
    }

    #[test]
    fn folder_fingerprints_change_with_any_image_and_ignore_duplicates() {
        let image = |name: &str, size| {
            let fingerprint = image_fingerprint(OsStr::new(name), size, (1_000, 0));
            (PathBuf::from(format!("/music/Album/{name}")), fingerprint)
        };
        let album = Path::new("/music/Album");
        let one = folder_fingerprints(vec![image("cover.jpg", 10)])[album];
        let twice = folder_fingerprints(vec![image("cover.jpg", 10), image("cover.jpg", 10)]);
        assert_eq!(twice[album], one);
        let resized = folder_fingerprints(vec![image("cover.jpg", 11)])[album];
        assert_ne!(resized, one);
        let renamed = folder_fingerprints(vec![image("folder.jpg", 10)])[album];
        assert_ne!(renamed, one);
        let both = folder_fingerprints(vec![image("folder.jpg", 10), image("cover.jpg", 10)]);
        let other_order =
            folder_fingerprints(vec![image("cover.jpg", 10), image("folder.jpg", 10)]);
        assert_eq!(both, other_order);
        assert_ne!(both[album], one);
    }
}
