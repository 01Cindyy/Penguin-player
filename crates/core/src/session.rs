//! The play queue as it was when the app quit, so the next run carries on from there.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::files::{read_text_limited, write_atomically};

/// A queue of every song in a very large library is a few megabytes.
const MAX_SESSION_BYTES: u64 = 64 * 1024 * 1024;

/// The queue and the place in it, stored as `session.json` in the data directory. Songs
/// are named by their files, as in playlists, so the queue outlives rescans.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    /// The queued songs in the order they were added (see
    /// [`crate::PlayQueue::items`]). `None` stands for a song that couldn't be named.
    #[serde(with = "crate::stored_path::list")]
    pub songs: Vec<Option<PathBuf>>,
    /// The play order while shuffled (see [`crate::PlayQueue::shuffled_order`]).
    pub order: Vec<usize>,
    /// The current song's place in the play order.
    pub current: usize,
    /// How far into the current song playback was.
    pub position_ms: u64,
}

impl Session {
    /// The session in `path`, or `None` if there is none or it can't be read: the app
    /// then starts with an empty queue, as it does the first time.
    pub fn load(path: &Path) -> Option<Self> {
        let text = match read_text_limited(path, MAX_SESSION_BYTES) {
            Ok(text) => text,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return None,
            Err(err) => {
                log::warn!("Could not read {}: {err}", path.display());
                return None;
            }
        };
        serde_json::from_str(&text)
            .inspect_err(|err| log::warn!("Invalid session in {}: {err}", path.display()))
            .ok()
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        let json = serde_json::to_vec(self).map_err(io::Error::other)?;
        write_atomically(path, &json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data/session.json");
        assert_eq!(Session::load(&path), None);

        let session = Session {
            songs: vec![
                Some("/music/Café/one.flac".into()),
                None,
                Some("/music/two.flac".into()),
            ],
            order: vec![2, 0, 1],
            current: 1,
            position_ms: 61_500,
        };
        session.save(&path).unwrap();
        assert_eq!(Session::load(&path), Some(session));
        let json = std::fs::read_to_string(&path).unwrap();
        let songs = r#""songs":["/music/Café/one.flac",null,"/music/two.flac"]"#;
        assert!(json.contains(songs), "{json}");
    }

    #[cfg(unix)]
    #[test]
    fn songs_with_names_that_are_not_unicode_round_trip() {
        use std::os::unix::ffi::OsStrExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        let session = Session {
            songs: vec![Some(
                std::ffi::OsStr::from_bytes(b"/music/caf\xe9.flac").into(),
            )],
            ..Session::default()
        };
        session.save(&path).unwrap();
        assert_eq!(Session::load(&path), Some(session));
    }

    #[test]
    fn unreadable_sessions_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(Session::load(&path), None);

        std::fs::write(&path, r#"{ "songs": ["/music/one.flac"] }"#).unwrap();
        let session = Session::load(&path).unwrap();
        assert_eq!(session.songs.len(), 1);
        assert_eq!((session.current, session.position_ms), (0, 0));
    }
}
