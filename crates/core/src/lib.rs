//! Shared building blocks for Penguin Player: the data model, user settings,
//! the play queue and small helpers every other crate depends on.
//!
//! Nothing in here touches GTK or audio APIs, so it stays portable and easy to test.

pub mod app_info;
pub mod collection;
pub mod events;
pub mod files;
pub mod format;
pub mod lyrics;
pub mod model;
pub mod paths;
pub mod queue;
pub mod search;
pub mod settings;
pub mod util;

pub use app_info::{APP_DIR_NAME, APP_ID, APP_NAME, APP_WEBSITE, VERSION};
pub use collection::{Album, AlbumKey, Artist};
pub use events::{PlaybackState, PlayerCommand, PlayerEvent};
pub use format::AudioFormat;
pub use lyrics::{LyricLine, Lyrics};
pub use model::{CoverId, Playlist, PlaylistEntry, PlaylistId, PlaylistItemId, Track, TrackId};
pub use paths::AppPaths;
pub use queue::{PlayQueue, RepeatMode};
pub use search::SearchQuery;
pub use settings::{ColorSchemePreference, CornerStyle, PluginPermissions, Settings, Transparency};
