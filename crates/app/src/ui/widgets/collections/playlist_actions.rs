//! What can be done with a whole playlist, as one action group shared by the
//! sidebar's context menu and the playlist page's menu.
//!
//! Every action takes the playlist id (`int64`) as its target, so one group serves
//! all playlists: insert [`action_group`] under [`GROUP`] on a widget, then build
//! menus for it with [`menu`].

use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;
use mp_core::{PlaylistId, TrackId};

use crate::context::AppContext;
use crate::model::PlaylistObject;
use crate::ui::{dialogs, track_actions};

/// The prefix to insert [`action_group`] under.
pub const GROUP: &str = "playlist";

type PlaylistAction = fn(&Rc<AppContext>, &PlaylistObject);

/// `play`, `shuffle`, `play-next`, `enqueue`, `rename` and `delete`.
pub fn action_group(ctx: &Rc<AppContext>) -> gio::SimpleActionGroup {
    let actions: [(&str, PlaylistAction); 6] = [
        ("play", play),
        ("shuffle", shuffle),
        ("play-next", play_next),
        ("enqueue", enqueue),
        ("rename", dialogs::rename_playlist),
        ("delete", dialogs::delete_playlist),
    ];
    let group = gio::SimpleActionGroup::new();
    for (name, run) in actions {
        let action = gio::SimpleAction::new(name, Some(track_actions::playlist_id_type()));
        let weak = Rc::downgrade(ctx);
        action.connect_activate(move |_, parameter| {
            let Some(ctx) = weak.upgrade() else { return };
            let playlist = parameter
                .and_then(|parameter| parameter.get::<i64>())
                .and_then(|id| ctx.store.playlist(PlaylistId(id)));
            if let Some(playlist) = playlist {
                run(&ctx, &playlist);
            }
        });
        group.add_action(&action);
    }
    group
}

/// The menu for playlist `id`: playback, then Rename… and Delete…. `with_play` adds
/// "Play" for places without a play button of their own.
pub fn menu(id: PlaylistId, with_play: bool) -> gio::Menu {
    let target = id.0.to_variant();
    let item = |label: &str, action: &str| {
        let item = gio::MenuItem::new(Some(label), None);
        item.set_action_and_target_value(Some(&format!("{GROUP}.{action}")), Some(&target));
        item
    };
    let playback = gio::Menu::new();
    if with_play {
        playback.append_item(&item("Play", "play"));
    }
    playback.append_item(&item("Play Next", "play-next"));
    playback.append_item(&item("Add to Queue", "enqueue"));
    let manage = gio::Menu::new();
    manage.append_item(&item("Rename…", "rename"));
    manage.append_item(&item("Delete…", "delete"));

    let menu = gio::Menu::new();
    menu.append_section(None, &playback);
    menu.append_section(None, &manage);
    menu
}

fn play(ctx: &Rc<AppContext>, playlist: &PlaylistObject) {
    if let Some(tracks) = playable_tracks(ctx, playlist) {
        ctx.player.play_all(tracks);
    }
}

fn shuffle(ctx: &Rc<AppContext>, playlist: &PlaylistObject) {
    if let Some(tracks) = playable_tracks(ctx, playlist) {
        ctx.player.shuffle_tracks(tracks);
    }
}

fn play_next(ctx: &Rc<AppContext>, playlist: &PlaylistObject) {
    if let Some(tracks) = playable_tracks(ctx, playlist) {
        ctx.player.play_next(&tracks);
        ctx.toast(&format!("“{}” will play next", playlist.name()));
    }
}

fn enqueue(ctx: &Rc<AppContext>, playlist: &PlaylistObject) {
    if let Some(tracks) = playable_tracks(ctx, playlist) {
        ctx.player.enqueue(&tracks);
        ctx.toast(&format!("Added “{}” to the queue", playlist.name()));
    }
}

/// The playlist's tracks that are in the library, or `None` (and a toast saying so).
fn playable_tracks(ctx: &AppContext, playlist: &PlaylistObject) -> Option<Vec<TrackId>> {
    let tracks = playlist.available_tracks();
    if tracks.is_empty() {
        ctx.toast(&format!("“{}” has no songs to play", playlist.name()));
        return None;
    }
    Some(tracks)
}
