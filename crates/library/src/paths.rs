//! Paths as the database sees them: one normalised spelling per file, and a lossless
//! conversion to and from the stored bytes.
//!
//! Unix paths are arbitrary bytes and are stored as they are. Windows paths are UTF-16
//! that may contain unpaired surrogates, so they are stored as WTF-8 (UTF-8 that can also
//! encode lone surrogates). Either way every path round-trips exactly, even if it isn't
//! valid Unicode.

use std::path::{Component, MAIN_SEPARATOR, Path, PathBuf};

/// `path` made absolute (against the current directory) and without repeated separators,
/// trailing separators or `.` components, so a file always gets the same database key
/// however its folder was written (`/music//a/./b.flac` becomes `/music/a/b.flac`).
/// Nothing is looked up on disk: symlinks and `..` are left alone.
pub(crate) fn normalize(path: &Path) -> PathBuf {
    std::path::absolute(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .components()
        .collect()
}

/// Whether the normalised `path` is `root` or inside it. A `..` below `root` counts as
/// outside, because it can lead anywhere.
pub(crate) fn is_inside(root: &Path, path: &Path) -> bool {
    path.strip_prefix(root).is_ok_and(|relative| {
        relative
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    })
}

/// Encodes a path for storage.
pub(crate) fn to_bytes(path: &Path) -> Vec<u8> {
    platform::to_bytes(path)
}

/// Decodes a path written by [`to_bytes`].
pub(crate) fn from_bytes(bytes: Vec<u8>) -> PathBuf {
    platform::from_bytes(bytes)
}

/// The byte range `[start, end)` that contains the encoding of every path inside `dir`
/// (but not `dir` itself). Lets "everything under this folder" use the path index.
pub(crate) fn children_range(dir: &Path) -> (Vec<u8>, Vec<u8>) {
    let separator = MAIN_SEPARATOR as u8;
    let mut start = to_bytes(dir);
    if start.last() != Some(&separator) {
        start.push(separator);
    }
    let mut end = start.clone();
    // The separator is ASCII, so incrementing the last byte can't overflow.
    *end.last_mut().expect("start ends with a separator") += 1;
    (start, end)
}

#[cfg(unix)]
mod platform {
    use std::ffi::OsString;
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::path::{Path, PathBuf};

    pub fn to_bytes(path: &Path) -> Vec<u8> {
        path.as_os_str().as_bytes().to_vec()
    }

    pub fn from_bytes(bytes: Vec<u8>) -> PathBuf {
        PathBuf::from(OsString::from_vec(bytes))
    }
}

#[cfg(windows)]
mod platform {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::path::{Path, PathBuf};

    pub fn to_bytes(path: &Path) -> Vec<u8> {
        super::wtf8::encode(path.as_os_str().encode_wide())
    }

    pub fn from_bytes(bytes: Vec<u8>) -> PathBuf {
        PathBuf::from(OsString::from_wide(&super::wtf8::decode(&bytes)))
    }
}

/// WTF-8: UTF-16 code units (possibly with unpaired surrogates) as generalised UTF-8.
/// Pure functions, so they are also compiled for tests on every platform.
#[cfg(any(windows, test))]
mod wtf8 {
    pub fn encode(units: impl Iterator<Item = u16>) -> Vec<u8> {
        let mut bytes = Vec::new();
        for unit in char::decode_utf16(units) {
            match unit {
                Ok(c) => bytes.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes()),
                Err(error) => {
                    // A lone surrogate is encoded like any other 3-byte code point.
                    let surrogate = error.unpaired_surrogate();
                    bytes.extend_from_slice(&[
                        0xE0 | (surrogate >> 12) as u8,
                        0x80 | ((surrogate >> 6) & 0x3F) as u8,
                        0x80 | (surrogate & 0x3F) as u8,
                    ]);
                }
            }
        }
        bytes
    }

    /// Invalid sequences (which `encode` never produces) decode to U+FFFD.
    pub fn decode(bytes: &[u8]) -> Vec<u16> {
        let mut units = Vec::with_capacity(bytes.len());
        let mut index = 0;
        while index < bytes.len() {
            let (code_point, len) = decode_one(&bytes[index..]);
            index += len;
            match char::from_u32(code_point) {
                Some(c) => units.extend_from_slice(c.encode_utf16(&mut [0; 2])),
                None if (0xD800..=0xDFFF).contains(&code_point) => units.push(code_point as u16),
                None => units.push(0xFFFD),
            }
        }
        units
    }

    /// Decodes the code point (surrogates allowed) at the start of `bytes`, returning it
    /// with its length in bytes.
    fn decode_one(bytes: &[u8]) -> (u32, usize) {
        const INVALID: (u32, usize) = (0xFFFD, 1);
        let first = bytes[0];
        let (len, initial) = match first {
            0x00..=0x7F => return (u32::from(first), 1),
            0xC0..=0xDF => (2, first & 0x1F),
            0xE0..=0xEF => (3, first & 0x0F),
            0xF0..=0xF4 => (4, first & 0x07),
            _ => return INVALID,
        };
        match bytes.get(1..len) {
            Some(tail) if tail.iter().all(|byte| byte & 0xC0 == 0x80) => {
                let code_point = tail.iter().fold(u32::from(initial), |acc, byte| {
                    (acc << 6) | u32::from(byte & 0x3F)
                });
                (code_point, len)
            }
            _ => INVALID,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_paths_round_trip() {
        let path = Path::new("/music/Kōji Arai/Night Transit/1-01 Shinjuku 2 a.m..m4a");
        assert_eq!(from_bytes(to_bytes(path)), path);
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_unix_paths_round_trip() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let path = Path::new(OsStr::from_bytes(b"/music/caf\xe9 \xff.flac"));
        assert_eq!(from_bytes(to_bytes(path)), path);
    }

    #[test]
    fn children_range_covers_only_children() {
        let dir = Path::new("/music/Album");
        let (start, end) = children_range(dir);
        let inside = |path: &str| {
            let bytes = to_bytes(Path::new(path));
            bytes >= start && bytes < end
        };
        assert!(inside("/music/Album/01.flac"));
        assert!(inside("/music/Album/CD 2/01.flac"));
        assert!(!inside("/music/Album"));
        assert!(!inside("/music/Album 2/01.flac"));
        assert!(!inside("/music/Albums/01.flac"));
        assert!(!inside("/music/Alb/01.flac"));
    }

    #[test]
    fn normalize_gives_one_spelling_per_path() {
        let expected = to_bytes(&normalize(Path::new("/music/Album/01.flac")));
        for spelling in ["/music//Album/./01.flac", "/music/./Album//01.flac/"] {
            assert_eq!(to_bytes(&normalize(Path::new(spelling))), expected);
        }
        let relative = normalize(Path::new("./music/Album"));
        assert!(relative.is_absolute());
        assert!(relative.ends_with("music/Album"));
    }

    #[test]
    fn parent_components_count_as_outside() {
        let root = Path::new("/music");
        assert!(is_inside(root, Path::new("/music")));
        assert!(is_inside(root, Path::new("/music/Album/01.flac")));
        assert!(!is_inside(root, Path::new("/music/../etc/x.flac")));
        assert!(!is_inside(root, Path::new("/music/Album/../../x.flac")));
        assert!(!is_inside(root, Path::new("/musical/x.flac")));
        assert!(!is_inside(root, Path::new("relative/x.flac")));
    }

    #[test]
    fn wtf8_round_trips_lone_surrogates() {
        let units: Vec<u16> = "Björk 🎵 ".encode_utf16().chain([0xD800, 0x41]).collect();
        let encoded = wtf8::encode(units.iter().copied());
        assert_eq!(wtf8::decode(&encoded), units);
        // Valid Unicode is plain UTF-8.
        let text = "Café 🎵";
        assert_eq!(wtf8::encode(text.encode_utf16()), text.as_bytes());
    }
}
