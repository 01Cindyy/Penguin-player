//! Reading files from folders that may contain anything, such as user themes and
//! plugins: only regular files, within a size limit, and never outside their folder.
//! Also writing the app's own files so a crash can't damage them.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

use crate::util::stable_hash;

/// Reads a regular file of at most `max_bytes`. Anything else is refused before it is
/// opened: a FIFO would block the reading thread, and a device could be endless. A
/// file that grows past the limit while it is read is refused too.
pub fn read_limited(path: &Path, max_bytes: u64) -> io::Result<Vec<u8>> {
    let too_large = || {
        io::Error::new(
            io::ErrorKind::FileTooLarge,
            format!("the file is larger than {} KiB", max_bytes / 1024),
        )
    };
    let metadata = std::fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    if metadata.len() > max_bytes {
        return Err(too_large());
    }
    let mut contents = Vec::new();
    File::open(path)?
        .take(max_bytes + 1)
        .read_to_end(&mut contents)?;
    if contents.len() as u64 > max_bytes {
        return Err(too_large());
    }
    Ok(contents)
}

/// [`read_limited`] for UTF-8 text files.
pub fn read_text_limited(path: &Path, max_bytes: u64) -> io::Result<String> {
    String::from_utf8(read_limited(path, max_bytes)?)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "the file is not valid UTF-8"))
}

/// The regular file `relative` names inside `dir`, resolved. `None` if it doesn't
/// exist or would leave `dir`: an absolute path, `..`, or a symlink pointing elsewhere.
pub fn file_inside(dir: &Path, relative: &Path) -> Option<PathBuf> {
    let plain = relative
        .components()
        .all(|component| matches!(component, Component::Normal(_)));
    if !plain || relative.as_os_str().is_empty() {
        return None;
    }
    let path = dir.join(relative).canonicalize().ok()?;
    let root = dir.canonicalize().ok()?;
    (path.starts_with(&root) && path.is_file()).then_some(path)
}

/// Replaces the file at `path` with `contents` so that a crash or a power cut leaves
/// the old file or the new one, never a partly written or empty one. The contents go
/// to `<path>.tmp`, are flushed to the disk, and only then take the file's place:
/// without the flush, some filesystems record the rename before the contents.
pub fn write_atomically(path: &Path, contents: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut temp = path.as_os_str().to_owned();
    temp.push(".tmp");
    let mut file = File::create(&temp)?;
    file.write_all(contents)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temp, path)
}

/// What the files kept in a cache for the song at `path` are named: 16 hex digits made
/// from the path, with any extension, so they are found again without a list of names.
pub fn song_cache_key(path: &Path) -> String {
    format!("{:016x}", stable_hash(path.as_os_str().as_encoded_bytes()))
}

/// Deletes the files in the cache folder `dir` that are named after songs (see
/// [`song_cache_key`]) other than those with `keys`: songs that have left the library.
/// Files named any other way are left alone. Returns how many were deleted.
pub fn remove_cache_files_of_other_songs(dir: &Path, keys: &HashSet<String>) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let key = name.to_str().and_then(|name| name.split('.').next());
        let outdated = key.is_some_and(|key| {
            key.len() == 16
                && key.bytes().all(|byte| byte.is_ascii_hexdigit())
                && !keys.contains(key)
        });
        if outdated && std::fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// Whether a file or folder name is hidden, by the Unix convention of a leading dot.
/// Scans, the folder watcher, and theme and plugin discovery all skip hidden names.
pub fn is_hidden_name(name: &OsStr) -> bool {
    name.as_encoded_bytes().first() == Some(&b'.')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_regular_files_are_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "hello").unwrap();
        assert_eq!(read_text_limited(&path, 5).unwrap(), "hello");
        let err = read_limited(&path, 4).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::FileTooLarge);
        let err = read_limited(dir.path(), 1024).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        std::fs::write(&path, [0xff, 0xfe]).unwrap();
        let err = read_text_limited(&path, 1024).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[cfg(unix)]
    #[test]
    fn devices_and_fifos_are_refused_without_blocking() {
        let err = read_limited(Path::new("/dev/zero"), 1024).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("plugin.json");
        std::os::unix::fs::symlink("/dev/zero", &link).unwrap();
        assert_eq!(
            read_limited(&link, 1024).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn files_inside_a_folder_stay_inside() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("theme");
        std::fs::create_dir_all(dir.join("css")).unwrap();
        std::fs::write(dir.join("css/main.css"), "").unwrap();
        std::fs::write(root.path().join("secret.css"), "").unwrap();

        let found = file_inside(&dir, Path::new("css/main.css")).unwrap();
        assert!(found.ends_with("theme/css/main.css"));
        for refused in ["../secret.css", "", "css", "missing.css", "./css/main.css"] {
            assert_eq!(file_inside(&dir, Path::new(refused)), None, "{refused}");
        }
        let absolute = root.path().join("secret.css");
        assert_eq!(file_inside(&dir, &absolute), None);
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&absolute, dir.join("link.css")).unwrap();
            assert_eq!(file_inside(&dir, Path::new("link.css")), None);
        }
    }

    #[test]
    fn atomic_writes_replace_the_file_and_leave_nothing_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new folder/settings.json");
        write_atomically(&path, b"first").unwrap();
        write_atomically(&path, b"second").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["settings.json"]);
    }

    #[test]
    fn cache_files_of_songs_that_left_are_removed() {
        let dir = tempfile::tempdir().unwrap();
        let kept = song_cache_key(Path::new("/music/kept.flac"));
        let gone = song_cache_key(Path::new("/music/gone.flac"));
        assert_eq!(kept.len(), 16);
        assert_ne!(kept, gone);
        let names = [
            kept.clone(),
            format!("{kept}.lrc"),
            gone.clone(),
            format!("{gone}.lrc"),
            format!("{gone}.lrc.tmp"),
            "notes.txt".to_owned(),
        ];
        for name in &names {
            std::fs::write(dir.path().join(name), "").unwrap();
        }

        let keys = HashSet::from([kept.clone()]);
        assert_eq!(remove_cache_files_of_other_songs(dir.path(), &keys), 3);
        let mut left: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [kept.clone(), format!("{kept}.lrc"), "notes.txt".to_owned()]
        );
        assert_eq!(
            remove_cache_files_of_other_songs(&dir.path().join("missing"), &keys),
            0
        );
    }

    #[test]
    fn hidden_names_start_with_a_dot() {
        assert!(is_hidden_name(OsStr::new(".git")));
        assert!(!is_hidden_name(OsStr::new("Album")));
        assert!(!is_hidden_name(OsStr::new("")));
    }
}
