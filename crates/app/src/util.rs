//! Small text helpers shared by the UI.

use std::path::Path;
use std::time::Duration;

pub use mp_core::util::format_count;
use mp_core::util::format_total_duration;

/// "1 song", "2,048 songs": a count followed by the singular or plural noun.
pub fn count_label(count: usize, singular: &str, plural: &str) -> String {
    let noun = if count == 1 { singular } else { plural };
    format!("{} {noun}", format_count(count))
}

/// "1 song", "2,048 songs".
pub fn song_count(count: usize) -> String {
    count_label(count, "song", "songs")
}

/// The last part of `path` for showing to the user, or all of it if it has none.
pub fn display_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// The total length of some songs, such as "48 min" or "1 hr 4 min".
pub fn total_length(duration_ms: u64) -> String {
    format_total_duration(Duration::from_millis(duration_ms))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pluralises() {
        assert_eq!(count_label(1, "song", "songs"), "1 song");
        assert_eq!(count_label(2048, "song", "songs"), "2,048 songs");
        assert_eq!(song_count(1), "1 song");
    }

    #[test]
    fn display_names_fall_back_to_the_whole_path() {
        assert_eq!(display_name(Path::new("/music/Song.flac")), "Song.flac");
        assert_eq!(display_name(Path::new("/")), "/");
    }
}
