//! A path in the app's JSON files (`#[serde(with = "crate::stored_path")]`): a string,
//! or, for a path that isn't valid Unicode (possible on Unix, e.g. Latin-1 folder names
//! from an old disk), its bytes as an array of numbers. serde's own `Path` support
//! refuses such paths, which would make every save fail.

use std::path::PathBuf;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum StoredPath {
    Text(String),
    Bytes(Vec<u8>),
}

pub fn serialize<S: Serializer>(path: &Option<PathBuf>, serializer: S) -> Result<S::Ok, S::Error> {
    stored(path).serialize(serializer)
}

pub fn deserialize<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<PathBuf>, D::Error> {
    Option::<StoredPath>::deserialize(deserializer).map(restored)
}

/// The same for a list of paths: `#[serde(with = "crate::stored_path::list")]`.
pub mod list {
    use super::*;

    pub fn serialize<S: Serializer>(
        paths: &[Option<PathBuf>],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(paths.iter().map(stored))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<Option<PathBuf>>, D::Error> {
        let paths = Vec::<Option<StoredPath>>::deserialize(deserializer)?;
        Ok(paths.into_iter().map(restored).collect())
    }
}

fn stored(path: &Option<PathBuf>) -> Option<StoredPath> {
    path.as_ref().map(|path| match path.to_str() {
        Some(text) => StoredPath::Text(text.to_owned()),
        None => to_bytes(path),
    })
}

fn restored(stored: Option<StoredPath>) -> Option<PathBuf> {
    stored.and_then(|stored| match stored {
        StoredPath::Text(text) => Some(PathBuf::from(text)),
        StoredPath::Bytes(bytes) => from_bytes(bytes),
    })
}

#[cfg(unix)]
fn to_bytes(path: &std::path::Path) -> StoredPath {
    use std::os::unix::ffi::OsStrExt;
    StoredPath::Bytes(path.as_os_str().as_bytes().to_vec())
}

#[cfg(unix)]
fn from_bytes(bytes: Vec<u8>) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    Some(std::ffi::OsString::from_vec(bytes).into())
}

/// Paths elsewhere are Unicode but for rare unpaired surrogates on Windows, which
/// are replaced.
#[cfg(not(unix))]
fn to_bytes(path: &std::path::Path) -> StoredPath {
    StoredPath::Text(path.to_string_lossy().into_owned())
}

#[cfg(not(unix))]
fn from_bytes(_bytes: Vec<u8>) -> Option<PathBuf> {
    None
}
