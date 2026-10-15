//! Playing and organising a whole album or artist: the `browse.*` actions behind
//! card menus, the cards' play buttons and the album and artist pages.

use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};
use mp_core::PlaylistId;

use super::Discography;
use crate::context::AppContext;
use crate::model::{AlbumObject, ArtistObject, LibraryStore, TrackObject};
use crate::ui::navigation::Route;
use crate::ui::track_actions;

/// The action group name the menus below refer to.
const GROUP: &str = "browse";

/// Something the browse pages show and the `browse.*` actions act on.
#[derive(Debug, Clone)]
pub enum BrowseItem {
    Album(AlbumObject),
    Artist(ArtistObject),
}

/// Whether a menu is for an album or an artist, which decides its items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    Album,
    Artist,
}

impl BrowseItem {
    pub fn kind(&self) -> ItemKind {
        match self {
            BrowseItem::Album(_) => ItemKind::Album,
            BrowseItem::Artist(_) => ItemKind::Artist,
        }
    }

    /// The album or artist in a list model item, if it is one.
    pub fn from_object(object: &glib::Object) -> Option<Self> {
        if let Some(album) = object.downcast_ref::<AlbumObject>() {
            return Some(BrowseItem::Album(album.clone()));
        }
        object
            .downcast_ref::<ArtistObject>()
            .map(|artist| BrowseItem::Artist(artist.clone()))
    }

    /// The page that shows it.
    pub fn route(&self) -> Route {
        match self {
            BrowseItem::Album(album) => Route::Album(album.key().clone()),
            BrowseItem::Artist(artist) => Route::Artist(artist.key().to_owned()),
        }
    }

    /// Its tracks in play order: album order, or an artist's whole [`Discography`].
    pub fn tracks(&self, store: &LibraryStore) -> Vec<TrackObject> {
        match self {
            BrowseItem::Album(album) => store.tracks_for(album.track_ids()),
            BrowseItem::Artist(artist) => Discography::of(store, artist).tracks,
        }
    }
}

/// Plays the item from the start (or from a random track when shuffle is on).
pub fn play(ctx: &AppContext, item: &BrowseItem) {
    let tracks = item.tracks(&ctx.store);
    if !tracks.is_empty() {
        ctx.player.play_all(track_actions::ids_of(&tracks));
    }
}

/// Turns shuffle on and plays the item in random order.
pub fn shuffle(ctx: &AppContext, item: &BrowseItem) {
    let tracks = item.tracks(&ctx.store);
    if !tracks.is_empty() {
        ctx.player.shuffle_tracks(track_actions::ids_of(&tracks));
    }
}

/// Adds the `browse.*` actions to `widget`. Each acts on what `target` returns at
/// the moment it runs, e.g. the card a menu was opened for.
pub fn install_actions(
    widget: &impl IsA<gtk::Widget>,
    ctx: &Rc<AppContext>,
    target: impl Fn() -> Option<BrowseItem> + 'static,
) {
    let target = Rc::new(target);
    let group = gio::SimpleActionGroup::new();
    let add = |name: &str, run: fn(&Rc<AppContext>, &BrowseItem)| {
        let action = gio::SimpleAction::new(name, None);
        let ctx = Rc::downgrade(ctx);
        let target = target.clone();
        action.connect_activate(move |_, _| {
            if let (Some(ctx), Some(item)) = (ctx.upgrade(), target()) {
                run(&ctx, &item);
            }
        });
        group.add_action(&action);
    };
    add("play", |ctx, item| play(ctx, item));
    add("shuffle", |ctx, item| shuffle(ctx, item));
    add("play-next", |ctx, item| {
        track_actions::play_next(ctx, &item.tracks(&ctx.store));
    });
    add("enqueue", |ctx, item| {
        track_actions::add_to_queue(ctx, &item.tracks(&ctx.store));
    });
    add("new-playlist", |ctx, item| {
        track_actions::add_to_new_playlist(ctx, &item.tracks(&ctx.store));
    });
    add("go-to-artist", |ctx, item| {
        let key = match item {
            BrowseItem::Album(album) => album.key().artist.clone(),
            BrowseItem::Artist(artist) => artist.key().to_owned(),
        };
        ctx.navigator.navigate(Route::Artist(key));
    });
    add("show-in-folder", |ctx, item| {
        if let Some(track) = item.tracks(&ctx.store).first() {
            track_actions::show_in_folder(ctx, track);
        }
    });

    let action = gio::SimpleAction::new("add-to-playlist", Some(track_actions::playlist_id_type()));
    let weak = Rc::downgrade(ctx);
    action.connect_activate(move |_, parameter| {
        let id = parameter.and_then(|parameter| parameter.get::<i64>());
        if let (Some(ctx), Some(item), Some(id)) = (weak.upgrade(), target(), id) {
            let tracks = item.tracks(&ctx.store);
            track_actions::add_to_playlist(&ctx, PlaylistId(id), &tracks);
        }
    });
    group.add_action(&action);

    widget.insert_action_group(GROUP, Some(&group));
}

/// The context menu of an album or artist card.
pub fn card_menu_model(ctx: &AppContext, kind: ItemKind) -> gio::Menu {
    let menu = gio::Menu::new();
    let playback = gio::Menu::new();
    playback.append(Some("Play"), Some("browse.play"));
    playback.append(Some("Shuffle"), Some("browse.shuffle"));
    playback.append(Some("Play Next"), Some("browse.play-next"));
    playback.append(Some("Add to Queue"), Some("browse.enqueue"));
    menu.append_section(None, &playback);
    menu.append_section(None, &playlist_section(ctx));
    if kind == ItemKind::Album {
        let browse = gio::Menu::new();
        browse.append(Some("Go to Artist"), Some("browse.go-to-artist"));
        menu.append_section(None, &browse);
    }
    menu
}

/// The "more" menu of an album or artist page, whose Play and Shuffle are buttons.
pub fn page_menu_model(ctx: &AppContext, kind: ItemKind) -> gio::Menu {
    let menu = gio::Menu::new();
    let playback = gio::Menu::new();
    playback.append(Some("Play Next"), Some("browse.play-next"));
    playback.append(Some("Add to Queue"), Some("browse.enqueue"));
    menu.append_section(None, &playback);
    menu.append_section(None, &playlist_section(ctx));
    if kind == ItemKind::Album {
        let files = gio::Menu::new();
        files.append(Some("Show in Folder"), Some("browse.show-in-folder"));
        menu.append_section(None, &files);
    }
    menu
}

/// Makes `button` open [`page_menu_model`], rebuilt on opening so it lists the current
/// playlists.
pub fn set_page_menu(button: &gtk::MenuButton, ctx: &Rc<AppContext>, kind: ItemKind) {
    let ctx = Rc::downgrade(ctx);
    button.set_create_popup_func(move |button| {
        if let Some(ctx) = ctx.upgrade() {
            button.set_menu_model(Some(&page_menu_model(&ctx, kind)));
        }
    });
}

/// "Add to Playlist", built when the menu opens so it lists the current playlists.
fn playlist_section(ctx: &AppContext) -> gio::Menu {
    let playlists = track_actions::playlist_submenu(
        ctx,
        &format!("{GROUP}.add-to-playlist"),
        &format!("{GROUP}.new-playlist"),
    );
    let section = gio::Menu::new();
    section.append_submenu(Some("Add to Playlist"), &playlists);
    section
}
