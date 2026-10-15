//! The full lists behind the search page's Songs, Albums and Artists chips.

use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

use super::ALBUM_CARDS;
use crate::context::AppContext;
use crate::model::{AlbumObject, ArtistObject, TrackObject};
use crate::ui::widgets::browse::{CardGrid, CardKind};
use crate::ui::widgets::{TrackColumn, TrackList};

/// The full lists behind the Songs, Albums and Artists chips. They are filled only
/// when shown, so typing never pays for lists nobody is looking at.
pub(super) struct FullLists {
    pub(super) songs: TrackList,
    song_model: gio::ListStore,
    pub(super) albums: CardGrid,
    album_model: gio::ListStore,
    pub(super) artists: CardGrid,
    artist_model: gio::ListStore,
}

impl FullLists {
    pub(super) fn new(ctx: &Rc<AppContext>) -> Self {
        let song_model = gio::ListStore::new::<TrackObject>();
        let songs = TrackList::builder(ctx)
            .columns(&[
                TrackColumn::Title,
                TrackColumn::Album,
                TrackColumn::Format,
                TrackColumn::Duration,
            ])
            .covers(true)
            .build(&song_model);
        songs.set_vexpand(true);

        let album_model = gio::ListStore::new::<AlbumObject>();
        let albums = CardGrid::builder(ctx, ALBUM_CARDS).build(&album_model);
        let artist_model = gio::ListStore::new::<ArtistObject>();
        let artists = CardGrid::builder(ctx, CardKind::Artists).build(&artist_model);
        Self {
            songs,
            song_model,
            albums,
            album_model,
            artists,
            artist_model,
        }
    }

    pub(super) fn show_songs(&self, songs: &[TrackObject]) {
        self.song_model.splice(0, self.song_model.n_items(), songs);
        if !songs.is_empty() {
            self.songs
                .column_view()
                .scroll_to(0, None, gtk::ListScrollFlags::NONE, None);
        }
    }

    pub(super) fn show_albums(&self, albums: &[AlbumObject]) {
        show_cards(&self.albums, &self.album_model, albums);
    }

    pub(super) fn show_artists(&self, artists: &[ArtistObject]) {
        show_cards(&self.artists, &self.artist_model, artists);
    }
}

/// Replaces the cards of a full list and scrolls back to its start.
fn show_cards(grid: &CardGrid, model: &gio::ListStore, items: &[impl IsA<glib::Object>]) {
    model.splice(0, model.n_items(), items);
    if !items.is_empty() {
        grid.grid_view()
            .scroll_to(0, gtk::ListScrollFlags::NONE, None);
    }
}
