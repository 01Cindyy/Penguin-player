use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::queue::RepeatMode;

pub const DEFAULT_THEME_ID: &str = "adwaita";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColorSchemePreference {
    #[default]
    System,
    Light,
    Dark,
}

/// How rounded the interface is. `Round` keeps the active theme's own corner radii;
/// the other two override them for every theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CornerStyle {
    Square,
    Slight,
    #[default]
    Round,
}

/// Window translucency, which works with any theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Transparency {
    pub enabled: bool,
    /// Ask the compositor to blur what shows through (where it supports that).
    pub blur: bool,
}

impl Default for Transparency {
    fn default() -> Self {
        Self {
            enabled: false,
            blur: true,
        }
    }
}

/// The window's size, and the sizes its panes were dragged to, in pixels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowState {
    pub width: i32,
    pub height: i32,
    pub maximized: bool,
    pub sidebar_width: i32,
    /// The sidebar was dragged so narrow that it shows only icons.
    pub sidebar_icons_only: bool,
    pub queue_width: i32,
    /// The visualiser's height, set by dragging the player bar's top edge.
    pub visualizer_height: i32,
    /// Now Playing shows the lyrics.
    pub now_playing_lyrics: bool,
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            width: 1180,
            height: 760,
            maximized: false,
            sidebar_width: 250,
            sidebar_icons_only: false,
            queue_width: 320,
            visualizer_height: 22,
            now_playing_lyrics: false,
        }
    }
}

/// What a plugin may do beyond its sandbox. Plugins ask for these in `plugin.json`, and
/// the user allows them when switching the plugin on.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginPermissions {
    /// Websites (host names, such as `lrclib.net`) it may fetch from over HTTPS.
    pub network: Vec<String>,
    /// It may show what is playing on the user's Discord profile.
    pub discord: bool,
}

impl PluginPermissions {
    pub fn is_empty(&self) -> bool {
        self.network.is_empty() && !self.discord
    }

    /// Whether these permissions include everything in `other`.
    pub fn covers(&self, other: &Self) -> bool {
        (self.discord || !other.discord)
            && other
                .network
                .iter()
                .all(|host| self.network.iter().any(|allowed| allowed == host))
    }

    /// What both allow.
    pub fn intersection(&self, other: &Self) -> Self {
        Self {
            network: self
                .network
                .iter()
                .filter(|host| other.network.contains(host))
                .cloned()
                .collect(),
            discord: self.discord && other.discord,
        }
    }
}

/// User preferences, stored as `settings.json` in the config directory.
///
/// Unknown or missing fields fall back to their defaults, so adding a field never
/// breaks an existing settings file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Root folder of the music library. `None` until the user picks one.
    #[serde(with = "stored_path")]
    pub music_folder: Option<PathBuf>,
    /// Watch the music folder and update the library automatically.
    pub watch_music_folder: bool,
    /// Id of the active theme (a built-in id or a user theme folder name).
    pub theme: String,
    pub color_scheme: ColorSchemePreference,
    pub corners: CornerStyle,
    pub transparency: Transparency,
    /// Crisper lines and a soft shadow between the bars and the page.
    pub defined_edges: bool,
    /// Show the playing song's bit depth, sample rate and bit rate, not just its format.
    pub quality_details: bool,
    /// Show the spectrum visualiser above the seek bar.
    pub visualizer: bool,
    /// Linear volume in `0.0..=1.0`.
    pub volume: f32,
    pub muted: bool,
    pub shuffle: bool,
    pub repeat: RepeatMode,
    pub gapless: bool,
    /// Ids of plugins the user has switched on.
    pub enabled_plugins: Vec<String>,
    /// What the user allowed each plugin to do when switching it on, by plugin id.
    pub plugin_permissions: BTreeMap<String, PluginPermissions>,
    /// The user's choices in each plugin's settings, by plugin id and setting key.
    /// Settings left at their defaults are missing.
    pub plugin_settings: BTreeMap<String, serde_json::Map<String, serde_json::Value>>,
    /// Save lyrics that plugins find as `.lrc` files beside the songs, rather than only
    /// in the cache.
    pub save_found_lyrics: bool,
    pub window: WindowState,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            music_folder: None,
            watch_music_folder: true,
            theme: DEFAULT_THEME_ID.to_owned(),
            color_scheme: ColorSchemePreference::System,
            corners: CornerStyle::Round,
            transparency: Transparency::default(),
            defined_edges: false,
            quality_details: false,
            visualizer: true,
            volume: 0.8,
            muted: false,
            shuffle: false,
            repeat: RepeatMode::Off,
            gapless: true,
            enabled_plugins: Vec::new(),
            plugin_permissions: BTreeMap::new(),
            plugin_settings: BTreeMap::new(),
            save_found_lyrics: false,
            window: WindowState::default(),
        }
    }
}

impl Settings {
    /// Loads settings, falling back to defaults if the file is missing. A file that
    /// cannot be parsed is kept as `settings.json.bak` so the user's edits aren't lost.
    pub fn load(path: &Path) -> Self {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Self::default(),
            Err(err) => {
                log::warn!("Could not read {}: {err}", path.display());
                return Self::default();
            }
        };
        match serde_json::from_str::<Settings>(&text) {
            Ok(settings) => settings.sanitized(),
            Err(err) => {
                log::warn!(
                    "Invalid settings in {}: {err}; using defaults",
                    path.display()
                );
                let backup = path.with_extension("json.bak");
                if let Err(err) = std::fs::rename(path, &backup) {
                    log::warn!("Could not back up {}: {err}", path.display());
                }
                Self::default()
            }
        }
    }

    /// Writes atomically (temp file + rename) so a crash can't leave a half-written file.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, json)?;
        std::fs::rename(&temp, path)
    }

    fn sanitized(mut self) -> Self {
        self.volume = if self.volume.is_finite() {
            self.volume.clamp(0.0, 1.0)
        } else {
            Settings::default().volume
        };
        if self.theme.trim().is_empty() {
            self.theme = DEFAULT_THEME_ID.to_owned();
        }
        self
    }
}

/// A path in `settings.json`: a string, or, for a path that isn't valid Unicode
/// (possible on Unix, e.g. Latin-1 folder names from an old disk), its bytes as an
/// array of numbers. serde's own `Path` support refuses such paths, which would make
/// every save fail.
mod stored_path {
    use std::path::PathBuf;

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize, Deserialize)]
    #[serde(untagged)]
    enum StoredPath {
        Text(String),
        Bytes(Vec<u8>),
    }

    pub fn serialize<S: Serializer>(
        path: &Option<PathBuf>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let stored = path.as_ref().map(|path| match path.to_str() {
            Some(text) => StoredPath::Text(text.to_owned()),
            None => to_bytes(path),
        });
        stored.serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<PathBuf>, D::Error> {
        let stored = Option::<StoredPath>::deserialize(deserializer)?;
        Ok(stored.and_then(|stored| match stored {
            StoredPath::Text(text) => Some(PathBuf::from(text)),
            StoredPath::Bytes(bytes) => from_bytes(bytes),
        }))
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let settings = Settings {
            music_folder: Some("/music".into()),
            volume: 0.5,
            repeat: RepeatMode::All,
            enabled_plugins: vec!["now-playing".into()],
            visualizer: false,
            defined_edges: true,
            quality_details: true,
            window: WindowState {
                sidebar_width: 300,
                sidebar_icons_only: true,
                ..WindowState::default()
            },
            ..Settings::default()
        };
        settings.save(&path).unwrap();
        assert_eq!(Settings::load(&path), settings);
    }

    #[test]
    fn music_folders_are_stored_as_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let settings = Settings {
            music_folder: Some("/music/Café".into()),
            ..Settings::default()
        };
        settings.save(&path).unwrap();
        let json = std::fs::read_to_string(&path).unwrap();
        assert!(json.contains(r#""music_folder": "/music/Café""#), "{json}");
        std::fs::write(&path, r#"{ "music_folder": null }"#).unwrap();
        assert_eq!(Settings::load(&path).music_folder, None);
    }

    #[cfg(unix)]
    #[test]
    fn music_folders_that_are_not_unicode_round_trip() {
        use std::os::unix::ffi::OsStrExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let settings = Settings {
            music_folder: Some(std::ffi::OsStr::from_bytes(b"/music/caf\xe9").into()),
            volume: 0.25,
            ..Settings::default()
        };
        settings.save(&path).unwrap();
        assert_eq!(Settings::load(&path), settings);
    }

    #[test]
    fn missing_fields_use_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{ "volume": 4.0, "repeat": "one" }"#).unwrap();
        let settings = Settings::load(&path);
        assert_eq!(settings.volume, 1.0);
        assert_eq!(settings.repeat, RepeatMode::One);
        assert!(settings.gapless);
        assert!(
            settings.visualizer,
            "settings from before the visualiser show it"
        );
        assert!(!settings.defined_edges && !settings.quality_details);
        assert_eq!(settings.theme, DEFAULT_THEME_ID);
    }

    #[test]
    fn plugin_permissions_cover_and_intersect() {
        let permissions = |hosts: &[&str], discord| PluginPermissions {
            network: hosts.iter().map(|host| host.to_string()).collect(),
            discord,
        };
        let granted = permissions(&["lrclib.net", "example.com"], false);
        assert!(granted.covers(&permissions(&["lrclib.net"], false)));
        assert!(granted.covers(&PluginPermissions::default()));
        assert!(!granted.covers(&permissions(&["lrclib.net"], true)));
        assert!(!granted.covers(&permissions(&["other.org"], false)));
        assert_eq!(
            granted.intersection(&permissions(&["example.com", "other.org"], true)),
            permissions(&["example.com"], false)
        );
        assert!(PluginPermissions::default().is_empty());
        assert!(!permissions(&[], true).is_empty());
    }

    #[test]
    fn corrupt_file_is_backed_up() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(Settings::load(&path), Settings::default());
        assert!(dir.path().join("settings.json.bak").exists());
    }
}
