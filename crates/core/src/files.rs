//! Reading files from folders that may contain anything, such as user themes and
//! plugins: only regular files, within a size limit, and never outside their folder.

use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

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
    fn hidden_names_start_with_a_dot() {
        assert!(is_hidden_name(OsStr::new(".git")));
        assert!(!is_hidden_name(OsStr::new("Album")));
        assert!(!is_hidden_name(OsStr::new("")));
    }
}
