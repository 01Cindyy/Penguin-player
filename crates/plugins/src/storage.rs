//! Persistent key/value storage for one plugin, saved as JSON in the plugin data
//! directory. Changes are written atomically, at most once per [`SAVE_INTERVAL`].

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mp_core::files::read_text_limited;
use serde_json::{Map, Value};

/// Plugins get a small store for settings and state, not a database.
pub(crate) const MAX_STORAGE_BYTES: usize = 1024 * 1024;

/// Changes are written at most this often, so a plugin storing values in a loop can't
/// keep the disk busy. Changes still waiting are written when the plugin is unloaded.
pub(crate) const SAVE_INTERVAL: Duration = Duration::from_secs(1);

/// How many tables deep a stored value may be. `serde_json` refuses to read files
/// nested deeper than 127 levels, so a deeper value could be saved but never loaded.
pub(crate) const MAX_DEPTH: usize = 64;

#[derive(Debug, thiserror::Error)]
pub(crate) enum StorageError {
    #[error("plugin storage is limited to {} KiB", MAX_STORAGE_BYTES / 1024)]
    TooLarge,
    #[error("could not save plugin storage: {0}")]
    Io(#[from] io::Error),
}

#[derive(Debug)]
pub(crate) struct Storage {
    path: PathBuf,
    values: Map<String, Value>,
    /// The file's new contents, while they wait for [`SAVE_INTERVAL`].
    unsaved: Option<Vec<u8>>,
    last_saved: Option<Instant>,
}

impl Storage {
    /// Loads the store at `path`. A missing file is an empty store; an unreadable one
    /// (invalid, too large, not a regular file) is kept as `<file>.bak` and replaced by
    /// an empty store.
    pub fn open(path: PathBuf) -> Self {
        let text = match read_text_limited(&path, MAX_STORAGE_BYTES as u64) {
            Ok(text) => text,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Self::empty(path),
            Err(err) => {
                log::warn!("Could not read plugin storage {}: {err}", path.display());
                set_aside(&path);
                return Self::empty(path);
            }
        };
        match serde_json::from_str(&text) {
            Ok(values) => Self::with_values(path, values),
            Err(err) => {
                log::warn!("Invalid plugin storage {}: {err}", path.display());
                set_aside(&path);
                Self::empty(path)
            }
        }
    }

    fn empty(path: PathBuf) -> Self {
        Self::with_values(path, Map::new())
    }

    fn with_values(path: PathBuf, values: Map<String, Value>) -> Self {
        Self {
            path,
            values,
            unsaved: None,
            last_saved: None,
        }
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.values.get(key)
    }

    /// Stores `value` under `key` (`None` removes it) and saves it now, or once
    /// [`SAVE_INTERVAL`] has passed since the last save. If the store would grow too
    /// large, it is left unchanged.
    pub fn set(&mut self, key: &str, value: Option<Value>) -> Result<(), StorageError> {
        let previous = match value {
            Some(value) => self.values.insert(key.to_owned(), value),
            None => self.values.remove(key),
        };
        match self.to_json() {
            Ok(json) => {
                self.unsaved = Some(json);
                self.save_if_due(Instant::now());
                Ok(())
            }
            Err(err) => {
                match previous {
                    Some(previous) => self.values.insert(key.to_owned(), previous),
                    None => self.values.remove(key),
                };
                Err(err)
            }
        }
    }

    /// When the changes not saved yet are due to be, if there are any.
    pub fn save_due(&self) -> Option<Instant> {
        self.unsaved.as_ref()?;
        Some(
            self.last_saved
                .map_or_else(Instant::now, |saved| saved + SAVE_INTERVAL),
        )
    }

    pub fn save_if_due(&mut self, now: Instant) {
        let due = self
            .last_saved
            .is_none_or(|saved| saved + SAVE_INTERVAL <= now);
        if due {
            self.save(now);
        }
    }

    fn to_json(&self) -> Result<Vec<u8>, StorageError> {
        let mut json = LimitedBuffer::new(MAX_STORAGE_BYTES);
        match serde_json::to_writer_pretty(&mut json, &self.values) {
            Ok(()) => Ok(json.bytes),
            Err(err) if err.is_io() => Err(StorageError::TooLarge),
            Err(err) => Err(StorageError::Io(io::Error::other(err))),
        }
    }

    /// Writes the unsaved changes. A failure is logged; the values stay in memory and
    /// the next change tries again.
    fn save(&mut self, now: Instant) {
        let Some(json) = self.unsaved.take() else {
            return;
        };
        self.last_saved = Some(now);
        if let Err(err) = write_atomically(&self.path, &json) {
            log::warn!(
                "Could not save plugin storage {}: {err}",
                self.path.display()
            );
        }
    }
}

impl Drop for Storage {
    fn drop(&mut self) {
        self.save(Instant::now());
    }
}

fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, bytes)?;
    std::fs::rename(&temp, path)
}

/// Collects written bytes up to `limit`; writing more fails. Indentation can make the
/// saved file many times larger than the values, so the limit is enforced while
/// writing rather than after.
struct LimitedBuffer {
    bytes: Vec<u8>,
    limit: usize,
}

impl LimitedBuffer {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
        }
    }
}

impl io::Write for LimitedBuffer {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if data.len() > self.limit - self.bytes.len() {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "plugin storage limit reached",
            ));
        }
        self.bytes.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Moves an unusable storage file out of the way, so saving doesn't overwrite what the
/// user may want to recover.
fn set_aside(path: &Path) {
    let backup = path.with_extension("json.bak");
    match std::fs::rename(path, &backup) {
        Ok(()) => log::warn!("Kept it as {}; starting empty", backup.display()),
        Err(err) => log::warn!("Could not back up {}: {err}", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn values_persist() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data/my.plugin.json");
        let mut storage = Storage::open(path.clone());
        assert_eq!(storage.get("volume"), None);
        storage.set("volume", Some(json!(0.5))).unwrap();
        storage
            .set("list", Some(json!([1, "two", { "three": 3 }])))
            .unwrap();
        storage.set("gone", Some(json!(true))).unwrap();
        storage.set("gone", None).unwrap();
        drop(storage);

        let reopened = Storage::open(path.clone());
        assert_eq!(reopened.get("volume"), Some(&json!(0.5)));
        assert_eq!(
            reopened.get("list"),
            Some(&json!([1, "two", { "three": 3 }]))
        );
        assert_eq!(reopened.get("gone"), None);
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn changes_are_saved_at_most_once_per_interval() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.json");
        let saved = || Storage::open(path.clone()).get("n").cloned();
        let mut storage = Storage::open(path.clone());
        storage.set("n", Some(json!(1))).unwrap();
        assert_eq!(saved(), Some(json!(1)), "the first change is saved at once");

        storage.set("n", Some(json!(2))).unwrap();
        storage.set("n", Some(json!(3))).unwrap();
        assert_eq!(saved(), Some(json!(1)));
        let due = storage.save_due().unwrap();
        storage.save_if_due(due - Duration::from_millis(1));
        assert_eq!(saved(), Some(json!(1)));
        storage.save_if_due(due);
        assert_eq!(saved(), Some(json!(3)));
        assert_eq!(storage.save_due(), None);

        storage.set("n", Some(json!(4))).unwrap();
        drop(storage);
        assert_eq!(
            saved(),
            Some(json!(4)),
            "unsaved changes are saved on unload"
        );
    }

    #[test]
    fn size_is_limited() {
        let dir = tempfile::tempdir().unwrap();
        let mut storage = Storage::open(dir.path().join("p.json"));
        storage.set("small", Some(json!("ok"))).unwrap();
        let huge = "x".repeat(MAX_STORAGE_BYTES);
        assert!(matches!(
            storage.set("huge", Some(json!(huge))),
            Err(StorageError::TooLarge)
        ));
        assert_eq!(storage.get("huge"), None);
        assert_eq!(storage.get("small"), Some(&json!("ok")));
    }

    /// Indentation makes a deeply nested list of small values over a hundred times
    /// larger than its compact JSON; writing stops as soon as the limit is reached
    /// instead of building the whole text first.
    #[test]
    fn saving_stops_at_the_size_limit() {
        let dir = tempfile::tempdir().unwrap();
        let mut storage = Storage::open(dir.path().join("p.json"));
        let mut nested = json!(vec![0; 300_000]);
        for _ in 0..MAX_DEPTH - 1 {
            nested = json!([nested]);
        }
        assert!(serde_json::to_vec(&nested).unwrap().len() < MAX_STORAGE_BYTES);
        assert!(matches!(
            storage.set("nested", Some(nested)),
            Err(StorageError::TooLarge)
        ));

        let mut buffer = LimitedBuffer::new(4);
        assert!(io::Write::write_all(&mut buffer, b"1234").is_ok());
        assert!(io::Write::write_all(&mut buffer, b"5").is_err());
        assert_eq!(buffer.bytes, b"1234");
    }

    #[test]
    fn oversized_file_is_set_aside_instead_of_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.json");
        let huge = format!(r#"{{ "big": "{}" }}"#, "x".repeat(MAX_STORAGE_BYTES));
        std::fs::write(&path, &huge).unwrap();
        let mut storage = Storage::open(path.clone());
        assert_eq!(storage.get("big"), None);
        storage.set("small", Some(json!(1))).unwrap();
        let backup = std::fs::read_to_string(path.with_extension("json.bak")).unwrap();
        assert_eq!(backup, huge, "the user's file is kept");
    }

    /// Only regular files are read: reading a device such as `/dev/zero` would never
    /// finish.
    #[cfg(unix)]
    #[test]
    fn special_files_are_not_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.json");
        std::os::unix::fs::symlink("/dev/zero", &path).unwrap();
        let storage = Storage::open(path.clone());
        assert_eq!(storage.get("anything"), None);
        assert!(path.with_extension("json.bak").is_symlink());
    }

    #[test]
    fn corrupt_file_is_backed_up() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.json");
        std::fs::write(&path, "{ nope").unwrap();
        let storage = Storage::open(path.clone());
        assert_eq!(storage.get("anything"), None);
        assert!(path.with_extension("json.bak").exists());
    }
}
