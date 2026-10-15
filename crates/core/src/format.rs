use serde::{Deserialize, Serialize};

/// File extensions the library scanner picks up (lowercase, without the dot).
pub const SUPPORTED_EXTENSIONS: &[&str] = &[
    "flac", "mp3", "m4a", "m4b", "mp4", "aac", "caf", "ogg", "oga", "opus", "wav", "wave", "aif",
    "aiff", "aifc", "mka", "webm",
];

pub fn is_supported_extension(ext: &str) -> bool {
    SUPPORTED_EXTENSIONS
        .iter()
        .any(|supported| supported.eq_ignore_ascii_case(ext))
}

/// The audio codec of a file, which is what users care about when sorting by "file type".
///
/// Containers are deliberately not modelled: an `.m4a` can hold AAC or ALAC and the
/// scanner reports whichever codec is actually inside.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioFormat {
    Flac,
    Alac,
    Wav,
    Aiff,
    Mp3,
    Aac,
    Opus,
    Vorbis,
    Unknown,
}

impl AudioFormat {
    pub const ALL: [AudioFormat; 9] = [
        AudioFormat::Flac,
        AudioFormat::Alac,
        AudioFormat::Wav,
        AudioFormat::Aiff,
        AudioFormat::Mp3,
        AudioFormat::Aac,
        AudioFormat::Opus,
        AudioFormat::Vorbis,
        AudioFormat::Unknown,
    ];

    /// Display label, e.g. "FLAC".
    pub fn label(self) -> &'static str {
        match self {
            AudioFormat::Flac => "FLAC",
            AudioFormat::Alac => "ALAC",
            AudioFormat::Wav => "WAV",
            AudioFormat::Aiff => "AIFF",
            AudioFormat::Mp3 => "MP3",
            AudioFormat::Aac => "AAC",
            AudioFormat::Opus => "Opus",
            AudioFormat::Vorbis => "Vorbis",
            AudioFormat::Unknown => "Unknown",
        }
    }

    /// Stable identifier used in the database and plugin API, e.g. "flac".
    pub fn id(self) -> &'static str {
        match self {
            AudioFormat::Flac => "flac",
            AudioFormat::Alac => "alac",
            AudioFormat::Wav => "wav",
            AudioFormat::Aiff => "aiff",
            AudioFormat::Mp3 => "mp3",
            AudioFormat::Aac => "aac",
            AudioFormat::Opus => "opus",
            AudioFormat::Vorbis => "vorbis",
            AudioFormat::Unknown => "unknown",
        }
    }

    /// Inverse of [`AudioFormat::id`]; unrecognised ids map to `Unknown`.
    pub fn from_id(id: &str) -> AudioFormat {
        AudioFormat::ALL
            .into_iter()
            .find(|format| format.id() == id)
            .unwrap_or(AudioFormat::Unknown)
    }

    pub fn is_lossless(self) -> bool {
        matches!(
            self,
            AudioFormat::Flac | AudioFormat::Alac | AudioFormat::Wav | AudioFormat::Aiff
        )
    }

    /// Best guess from a file extension. MP4-family files report AAC here;
    /// the scanner refines that to ALAC when the stream says so.
    pub fn from_extension(ext: &str) -> Option<AudioFormat> {
        let format = match ext.to_ascii_lowercase().as_str() {
            "flac" => AudioFormat::Flac,
            "mp3" => AudioFormat::Mp3,
            "m4a" | "m4b" | "mp4" | "aac" => AudioFormat::Aac,
            "caf" => AudioFormat::Alac,
            "ogg" | "oga" => AudioFormat::Vorbis,
            "opus" => AudioFormat::Opus,
            "wav" | "wave" => AudioFormat::Wav,
            "aif" | "aiff" | "aifc" => AudioFormat::Aiff,
            "mka" | "webm" => AudioFormat::Unknown,
            _ => return None,
        };
        Some(format)
    }
}

impl std::fmt::Display for AudioFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip() {
        for format in AudioFormat::ALL {
            assert_eq!(AudioFormat::from_id(format.id()), format);
        }
        assert_eq!(AudioFormat::from_id("nonsense"), AudioFormat::Unknown);
    }

    #[test]
    fn extensions() {
        assert_eq!(AudioFormat::from_extension("FLAC"), Some(AudioFormat::Flac));
        assert_eq!(AudioFormat::from_extension("m4a"), Some(AudioFormat::Aac));
        assert_eq!(AudioFormat::from_extension("txt"), None);
        assert!(is_supported_extension("Opus"));
        assert!(!is_supported_extension("jpg"));
    }
}
