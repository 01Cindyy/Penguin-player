//! Every icon the app shows. They are bundled (`data/icons/`, see its README) under
//! names of their own, so they look the same with any icon theme, on systems without
//! one, and a system theme can't replace them.

use gtk::{gdk, gio, glib};

/// The resource path `build.rs` puts the icons under.
const RESOURCE_PATH: &str = "/penguin-player/icons";

pub const PLAY: &str = "mp-play-symbolic";
pub const PAUSE: &str = "mp-pause-symbolic";
pub const SKIP_BACKWARD: &str = "mp-skip-backward-symbolic";
pub const SKIP_FORWARD: &str = "mp-skip-forward-symbolic";
pub const SHUFFLE: &str = "mp-shuffle-symbolic";
pub const REPEAT: &str = "mp-repeat-symbolic";
pub const REPEAT_ONE: &str = "mp-repeat-one-symbolic";
/// The queue plays in order: neither shuffle nor repeat is on.
pub const IN_ORDER: &str = "mp-in-order-symbolic";
pub const VOLUME_HIGH: &str = "mp-volume-high-symbolic";
pub const VOLUME_MEDIUM: &str = "mp-volume-medium-symbolic";
pub const VOLUME_LOW: &str = "mp-volume-low-symbolic";
pub const VOLUME_MUTED: &str = "mp-volume-muted-symbolic";
/// Marks the track that is playing.
pub const NOW_PLAYING: &str = VOLUME_HIGH;
pub const QUEUE: &str = "mp-queue-symbolic";
pub const LYRICS: &str = "mp-lyrics-symbolic";

/// A music note: songs, and art that is missing.
pub const SONG: &str = "mp-song-symbolic";
pub const ALBUM: &str = "mp-album-symbolic";
/// One disc of a multi-disc album.
pub const DISC: &str = "mp-disc-symbolic";
pub const ARTIST: &str = "mp-artist-symbolic";
pub const FORMATS: &str = "mp-formats-symbolic";
/// The Now Playing page.
pub const HEADPHONES: &str = "mp-headphones-symbolic";
pub const MUSIC_FOLDER: &str = "mp-music-folder-symbolic";
pub const OPEN_FOLDER: &str = "mp-open-folder-symbolic";

pub const ADD: &str = "mp-add-symbolic";
pub const REMOVE: &str = "mp-remove-symbolic";
pub const TRASH: &str = "mp-trash-symbolic";
pub const BACK: &str = "mp-back-symbolic";
pub const MENU: &str = "mp-menu-symbolic";
/// "More options" menus of pages and rows.
pub const MORE: &str = "mp-more-symbolic";
pub const SEARCH: &str = "mp-search-symbolic";
pub const SORT_DESCENDING: &str = "mp-sort-descending-symbolic";
pub const REFRESH: &str = "mp-refresh-symbolic";
pub const CHECK: &str = "mp-check-symbolic";
pub const EXPAND: &str = "mp-expand-symbolic";

pub const PREFERENCES: &str = "mp-preferences-symbolic";
/// The preferences' pages.
pub const GENERAL: &str = "mp-general-symbolic";
pub const APPEARANCE: &str = "mp-appearance-symbolic";
pub const PLUGINS: &str = "mp-plugins-symbolic";
/// Documentation, such as the plugin guide.
pub const GUIDE: &str = "mp-guide-symbolic";
pub const WARNING: &str = "mp-warning-symbolic";
/// Files that are missing.
pub const UNAVAILABLE: &str = "mp-unavailable-symbolic";

/// Makes the bundled icons available to every widget on `display`.
pub fn register(display: &gdk::Display) {
    match bundle() {
        Ok(resource) => {
            gio::resources_register(&resource);
            gtk::IconTheme::for_display(display).add_resource_path(RESOURCE_PATH);
        }
        Err(err) => log::error!("Couldn’t load the bundled icons: {err}"),
    }
}

fn bundle() -> Result<gio::Resource, glib::Error> {
    let data = include_bytes!(concat!(env!("OUT_DIR"), "/icons.gresource"));
    gio::Resource::from_data(&glib::Bytes::from_static(data))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::Path;

    use super::*;

    /// Every string literal ending in `-symbolic` in the source files under `dir`.
    fn icon_names_in(dir: &Path, names: &mut BTreeSet<String>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                icon_names_in(&path, names);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let source = fs::read_to_string(&path).unwrap();
                for (end, suffix) in source.match_indices("-symbolic\"") {
                    let start = source[..end].rfind('"').unwrap() + 1;
                    names.insert(source[start..end + suffix.len() - 1].to_owned());
                }
            }
        }
    }

    #[test]
    fn every_icon_named_is_bundled_and_every_bundled_icon_is_named() {
        let mut named = BTreeSet::new();
        icon_names_in(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut named,
        );
        // The app icon is named by the app id (window and About dialog), not a literal.
        named.insert(mp_core::APP_ID.to_owned());

        let folder = format!("{RESOURCE_PATH}/scalable/actions");
        let bundled: BTreeSet<String> = bundle()
            .unwrap()
            .enumerate_children(&folder, gio::ResourceLookupFlags::NONE)
            .unwrap()
            .iter()
            .filter_map(|file| file.strip_suffix(".svg").map(str::to_owned))
            .collect();
        assert_eq!(named, bundled);
    }
}
