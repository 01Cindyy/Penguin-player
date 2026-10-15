//! Sandboxed Lua plugins.
//!
//! A plugin is a folder in the plugins directory containing `plugin.json` and a Lua
//! entry script. All plugins run on one dedicated host thread, each in its own Lua
//! state with a restricted standard library. They react to [`PlayerEvent`]s and talk
//! back to the app through [`HostEvent`]s, so a misbehaving plugin can never block the UI.
//!
//! The Lua API is documented for plugin authors in `docs/plugins.md`.
//!
//! Module overview:
//! - `manifest`: `plugin.json`, discovery and validation.
//! - `bundled`: the plugins shipped inside the app.
//! - `host`: the [`PluginHost`] handle and the host thread's event loop.
//! - `plugin`: one loaded plugin (Lua state, handlers, timers, error counting).
//! - `sandbox`: the restricted Lua environment, time and memory limits, `require`.
//! - `api`: the `mp` table plugins script against.
//! - `events`: event names and the Lua values handlers receive.
//! - `registry`: the handlers and timers a plugin has registered.
//! - `snapshot`: the player state the `mp.player` getters answer from.
//! - `storage`: the JSON file behind `mp.storage`.
//! - `http`: web requests, for plugins allowed to make them.
//! - `discord`: Discord Rich Presence, for plugins allowed to use it.
//! - `lyrics`: the app's requests for lyrics, answered by plugins.
//! - `rate_limit`: caps on notifications, commands, log messages and errors per plugin.
//! - `errors`: short, located error messages for the plugin manager.
//! - `text`: text truncation. Files are read with `mp_core::files`.
//!
//! [`PlayerEvent`]: mp_core::PlayerEvent

mod api;
mod bundled;
mod discord;
mod errors;
mod events;
mod host;
mod http;
mod lyrics;
mod manifest;
mod plugin;
mod rate_limit;
mod registry;
mod sandbox;
mod snapshot;
mod storage;
mod text;

pub use discord::{
    Activity, ActivityKind, DiscordConfig, DiscordPresence, NoPresence, Presence, StatusDisplay,
};
pub use host::{HostConfig, HostEvent, PluginHost, Services};
pub use http::{HttpClient, HttpRequest, HttpResponse, Offline, WebClient};
pub use lyrics::{LyricsSearch, OwnLyrics};
pub use manifest::{
    MANIFEST_FILE, PluginInfo, PluginManifest, PluginSource, SettingChoice, SettingKind,
    SettingSpec, discover,
};
pub use sandbox::{CALLBACK_TIME_LIMIT, MEMORY_LIMIT};

/// Version of the Lua API exposed to plugins. Plugins declaring a newer version are
/// refused with an explanatory message. Version 2 added permissions (`mp.http`,
/// `mp.discord`), settings, `mp.json` and `mp.lyrics`.
pub const API_VERSION: u32 = 2;
