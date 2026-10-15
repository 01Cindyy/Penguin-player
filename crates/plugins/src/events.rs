//! Plugin event names and the Lua values passed to their handlers.

use mlua::{IntoLuaMulti, Lua, MultiValue, Table};
use mp_core::{PlayerEvent, Track};

/// Events a plugin can subscribe to with `mp.on(name, handler)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum EventKind {
    TrackChanged,
    StateChanged,
    Seeked,
    VolumeChanged,
    ShuffleChanged,
    RepeatChanged,
    LibraryUpdated,
    /// The user changed one of the plugin's settings.
    SettingChanged,
    /// Sent once before the plugin is unloaded (app exit, disable or reload).
    Shutdown,
}

impl EventKind {
    pub const ALL: [EventKind; 9] = [
        EventKind::TrackChanged,
        EventKind::StateChanged,
        EventKind::Seeked,
        EventKind::VolumeChanged,
        EventKind::ShuffleChanged,
        EventKind::RepeatChanged,
        EventKind::LibraryUpdated,
        EventKind::SettingChanged,
        EventKind::Shutdown,
    ];

    pub fn name(self) -> &'static str {
        match self {
            EventKind::TrackChanged => "track_changed",
            EventKind::StateChanged => "state_changed",
            EventKind::Seeked => "seeked",
            EventKind::VolumeChanged => "volume_changed",
            EventKind::ShuffleChanged => "shuffle_changed",
            EventKind::RepeatChanged => "repeat_changed",
            EventKind::LibraryUpdated => "library_updated",
            EventKind::SettingChanged => "setting_changed",
            EventKind::Shutdown => "shutdown",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }

    pub fn of(event: &PlayerEvent) -> Self {
        match event {
            PlayerEvent::TrackChanged(_) => EventKind::TrackChanged,
            PlayerEvent::StateChanged(_) => EventKind::StateChanged,
            PlayerEvent::Seeked { .. } => EventKind::Seeked,
            PlayerEvent::VolumeChanged { .. } => EventKind::VolumeChanged,
            PlayerEvent::ShuffleChanged(_) => EventKind::ShuffleChanged,
            PlayerEvent::RepeatChanged(_) => EventKind::RepeatChanged,
            PlayerEvent::LibraryUpdated { .. } => EventKind::LibraryUpdated,
        }
    }
}

/// The arguments a handler receives for `event`.
pub(crate) fn event_args(lua: &Lua, event: &PlayerEvent) -> mlua::Result<MultiValue> {
    match event {
        PlayerEvent::TrackChanged(track) => track
            .as_deref()
            .map(|track| track_table(lua, track))
            .transpose()?
            .into_lua_multi(lua),
        PlayerEvent::StateChanged(state) => state.id().into_lua_multi(lua),
        PlayerEvent::Seeked { position } => duration_ms(position.as_millis()).into_lua_multi(lua),
        PlayerEvent::VolumeChanged { volume, muted } => {
            (f64::from(*volume), *muted).into_lua_multi(lua)
        }
        PlayerEvent::ShuffleChanged(shuffle) => shuffle.into_lua_multi(lua),
        PlayerEvent::RepeatChanged(mode) => mode.id().into_lua_multi(lua),
        PlayerEvent::LibraryUpdated { track_count } => i64::try_from(*track_count)
            .unwrap_or(i64::MAX)
            .into_lua_multi(lua),
    }
}

/// A track as the Lua table documented in `docs/plugins.md`.
pub(crate) fn track_table(lua: &Lua, track: &Track) -> mlua::Result<Table> {
    let table = lua.create_table_with_capacity(0, 16)?;
    table.set("id", track.id.0)?;
    table.set("title", track.title.as_str())?;
    table.set("artist", track.artist.as_str())?;
    table.set("album", track.album.as_str())?;
    table.set("album_artist", track.album_artist.as_str())?;
    table.set("genre", track.genre.as_str())?;
    table.set("year", track.year)?;
    table.set("track_number", track.track_number)?;
    table.set("disc_number", track.disc_number)?;
    table.set("duration_ms", duration_ms(u128::from(track.duration_ms)))?;
    table.set("format", track.format.id())?;
    table.set("sample_rate", track.sample_rate)?;
    table.set("bit_depth", track.bit_depth)?;
    table.set("channels", track.channels)?;
    table.set("bitrate_kbps", track.bitrate_kbps)?;
    table.set("path", track.path.to_string_lossy().as_ref())?;
    Ok(table)
}

/// Milliseconds as a Lua integer, saturating instead of wrapping.
pub(crate) fn duration_ms(ms: u128) -> i64 {
    i64::try_from(ms).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for kind in EventKind::ALL {
            assert_eq!(EventKind::from_name(kind.name()), Some(kind));
        }
        assert_eq!(EventKind::from_name("nope"), None);
    }
}
