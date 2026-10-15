//! What can be done with a selection of tracks. Shared by every track context menu
//! so "Add to playlist" behaves the same on every page.

use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};
use mp_core::{PlaylistId, TrackId};

use crate::context::AppContext;
use crate::model::TrackObject;
use crate::ui::dialogs;
use crate::ui::navigation::Route;
use crate::util::song_count;

/// Track ids of `tracks`, in order.
pub fn ids_of(tracks: &[TrackObject]) -> Vec<TrackId> {
    tracks.iter().map(TrackObject::track_id).collect()
}

pub fn play_next(ctx: &AppContext, tracks: &[TrackObject]) {
    ctx.player.play_next(&ids_of(tracks));
    ctx.toast(&format!("{} will play next", describe(tracks)));
}

pub fn add_to_queue(ctx: &AppContext, tracks: &[TrackObject]) {
    ctx.player.enqueue(&ids_of(tracks));
    ctx.toast(&format!("Added {} to the queue", describe(tracks)));
}

pub fn add_to_playlist(ctx: &AppContext, playlist: PlaylistId, tracks: &[TrackObject]) {
    let ids = ids_of(tracks);
    let added = ctx.edit_library("add to the playlist", |library| {
        library.add_to_playlist(playlist, &ids)
    });
    if added.is_none() {
        return;
    }
    ctx.refresh_playlists();
    let name = ctx
        .store
        .playlist(playlist)
        .map_or_else(|| "the playlist".to_owned(), |p| format!("“{}”", p.name()));
    ctx.toast(&format!("Added {} to {name}", describe(tracks)));
}

/// Asks for a name, then creates a playlist containing `tracks`.
pub fn add_to_new_playlist(ctx: &Rc<AppContext>, tracks: &[TrackObject]) {
    let ids = ids_of(tracks);
    let count = describe(tracks);
    let weak = Rc::downgrade(ctx);
    dialogs::new_playlist(ctx, ids, move |_, name| {
        if let Some(ctx) = weak.upgrade() {
            ctx.toast(&format!("Added {count} to “{name}”"));
        }
    });
}

fn go_to_album(ctx: &AppContext, track: &TrackObject) {
    ctx.navigator.navigate(Route::Album(track.album_key()));
}

fn go_to_artist(ctx: &AppContext, track: &TrackObject) {
    ctx.navigator.navigate(Route::Artist(track.artist_key()));
}

/// Opens the file manager with the track's file selected.
pub fn show_in_folder(ctx: &AppContext, track: &TrackObject) {
    let file = gio::File::for_path(&track.track().path);
    let launcher = gtk::FileLauncher::new(Some(&file));
    let window = ctx.window();
    launcher.open_containing_folder(window.as_ref(), gio::Cancellable::NONE, |result| {
        if let Err(err) = result
            && !err.matches(gtk::DialogError::Dismissed)
        {
            log::warn!("Could not show the file in its folder: {err}");
        }
    });
}

/// "“Song title”" for one track, "3 songs" for several.
pub fn describe(tracks: &[TrackObject]) -> String {
    match tracks {
        [track] => format!("“{}”", track.title()),
        _ => song_count(tracks.len()),
    }
}

/// The songs a menu was opened for.
pub struct MenuSongs {
    /// What the playback and playlist items act on, e.g. every selected song.
    pub songs: Vec<TrackObject>,
    /// The song the menu was opened on, whose album, artist or folder "Go to" opens.
    pub opened_on: TrackObject,
}

/// Adds the `tracks.*` actions of [`context_menu`] to `widget`. Each acts on what
/// `target` returns at the moment it runs; "Play" runs `play`, as playing depends on
/// where the songs are listed.
pub fn install_actions(
    widget: &impl IsA<gtk::Widget>,
    ctx: &Rc<AppContext>,
    target: impl Fn() -> Option<MenuSongs> + 'static,
    play: impl Fn() + 'static,
) {
    let target = Rc::new(target);
    let group = gio::SimpleActionGroup::new();
    let add = |name: &str, run: fn(&Rc<AppContext>, &MenuSongs)| {
        let action = gio::SimpleAction::new(name, None);
        let ctx = Rc::downgrade(ctx);
        let target = target.clone();
        action.connect_activate(move |_, _| {
            if let (Some(ctx), Some(menu)) = (ctx.upgrade(), target()) {
                run(&ctx, &menu);
            }
        });
        group.add_action(&action);
    };
    add("play-next", |ctx, menu| play_next(ctx, &menu.songs));
    add("enqueue", |ctx, menu| add_to_queue(ctx, &menu.songs));
    add("new-playlist", |ctx, menu| {
        add_to_new_playlist(ctx, &menu.songs)
    });
    add("go-to-album", |ctx, menu| go_to_album(ctx, &menu.opened_on));
    add("go-to-artist", |ctx, menu| {
        go_to_artist(ctx, &menu.opened_on)
    });
    add("show-in-folder", |ctx, menu| {
        show_in_folder(ctx, &menu.opened_on)
    });

    let action = gio::SimpleAction::new("add-to-playlist", Some(playlist_id_type()));
    let weak = Rc::downgrade(ctx);
    action.connect_activate(move |_, parameter| {
        let id = parameter.and_then(|parameter| parameter.get::<i64>());
        if let (Some(ctx), Some(menu), Some(id)) = (weak.upgrade(), target(), id) {
            add_to_playlist(&ctx, PlaylistId(id), &menu.songs);
        }
    });
    group.add_action(&action);

    let action = gio::SimpleAction::new("play", None);
    action.connect_activate(move |_, _| play());
    group.add_action(&action);

    widget.insert_action_group("tracks", Some(&group));
}

/// The context menu of songs: playback, "Add to Playlist", then going to the song's
/// album, artist or folder, for the actions of [`install_actions`]. Build it when the
/// menu opens, so it lists the current playlists.
pub fn context_menu(ctx: &AppContext) -> gio::Menu {
    let playback = gio::Menu::new();
    playback.append(Some("Play"), Some("tracks.play"));
    playback.append(Some("Play Next"), Some("tracks.play-next"));
    playback.append(Some("Add to Queue"), Some("tracks.enqueue"));

    let playlists = playlist_submenu(ctx, "tracks.add-to-playlist", "tracks.new-playlist");
    let organise = gio::Menu::new();
    organise.append_submenu(Some("Add to Playlist"), &playlists);

    let browse = gio::Menu::new();
    browse.append(Some("Go to Album"), Some("tracks.go-to-album"));
    browse.append(Some("Go to Artist"), Some("tracks.go-to-artist"));
    browse.append(Some("Show in Folder"), Some("tracks.show-in-folder"));

    let menu = gio::Menu::new();
    menu.append_section(None, &playback);
    menu.append_section(None, &organise);
    menu.append_section(None, &browse);
    menu
}

/// Builds the "Add to Playlist" submenu: one entry per playlist (targeting
/// `add_action` with the playlist id) and "New Playlist…" (`new_action`).
pub fn playlist_submenu(ctx: &AppContext, add_action: &str, new_action: &str) -> gio::Menu {
    let menu = gio::Menu::new();
    let playlists = gio::Menu::new();
    for playlist in ctx
        .store
        .playlists()
        .iter::<crate::model::PlaylistObject>()
        .filter_map(Result::ok)
    {
        let item = gio::MenuItem::new(Some(&playlist.name()), None);
        item.set_action_and_target_value(
            Some(add_action),
            Some(&playlist.playlist_id().0.to_variant()),
        );
        playlists.append_item(&item);
    }
    menu.append_section(None, &playlists);
    menu.append(Some("New Playlist…"), Some(new_action));
    menu
}

/// The variant type of the playlist id targeted by [`playlist_submenu`] items.
pub fn playlist_id_type() -> &'static glib::VariantTy {
    glib::VariantTy::INT64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::test_track;

    #[test]
    fn describes_one_song_by_title() {
        let tracks: Vec<TrackObject> = (1..=2)
            .map(|id| TrackObject::new(test_track(id, &format!("Song {id}"), "a", "b")))
            .collect();
        assert_eq!(describe(&tracks[..1]), "“Song 1”");
        assert_eq!(describe(&tracks), "2 songs");
    }
}
