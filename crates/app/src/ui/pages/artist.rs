//! One album artist: a header with their image and stats, then their albums, their
//! songs and the albums by others they appear on.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use crate::context::AppContext;
use crate::model::{AlbumObject, ArtistObject, TrackObject};
use crate::ui::icons;
use crate::ui::track_actions;
use crate::ui::widgets::browse::{
    self, AlbumCardDetail, ArtistImage, BrowseItem, CardGrid, CardKind, CollapsibleItems,
    DetailPage, Discography, ItemKind, artist_stats, has_guest_artists,
};
use crate::ui::widgets::{CoverSize, PageHeader, TrackColumn, TrackList};
use crate::ui::{disconnect_on_destroy, keep_alive};
use crate::util::total_length;

/// Album cards listed per section until "Show All": full rows at 2, 3, 4 or 6 columns.
const COLLAPSED_ALBUMS: usize = 12;
const COLLAPSED_SONGS: usize = 10;

/// `key` is the artist's normalised name (see [`crate::model::ArtistObject::key`]).
pub fn build(ctx: &Rc<AppContext>, key: &str) -> adw::NavigationPage {
    let (page, navigation_page) = ArtistPage::new(ctx, key);
    let page = Rc::new(page);
    page.connect_signals();
    page.refresh();

    navigation_page.add_css_class("mp-artist-page");
    let weak = Rc::downgrade(&page);
    browse::install_actions(&navigation_page, ctx, move || {
        weak.upgrade().and_then(|page| page.item())
    });
    let weak = Rc::downgrade(&page);
    let handler = ctx.store.connect_tracks_changed(move |_| {
        if let Some(page) = weak.upgrade() {
            page.refresh();
        }
    });
    disconnect_on_destroy(&navigation_page, &ctx.store, handler);
    keep_alive(&navigation_page, page);
    navigation_page
}

/// The page's state, owned by its content widget (see [`keep_alive`]).
struct ArtistPage {
    ctx: Rc<AppContext>,
    key: String,
    /// The artist shown; `None` once they have left the library.
    artist: RefCell<Option<ArtistObject>>,
    detail: DetailPage,
    header: PageHeader,
    image: ArtistImage,
    play: gtk::Button,
    shuffle: gtk::Button,
    albums: Rc<CollapsibleItems<AlbumObject>>,
    albums_section: gtk::Box,
    /// Every song by the artist, in the order Play plays them.
    songs: Rc<CollapsibleItems<TrackObject>>,
    songs_section: gtk::Box,
    /// Holds the song list, which is rebuilt when it starts or stops showing artists.
    songs_slot: adw::Bin,
    songs_show_artists: Cell<Option<bool>>,
    appearances: Rc<CollapsibleItems<AlbumObject>>,
    appearances_section: gtk::Box,
}

impl ArtistPage {
    /// The page's state and the navigation page that shows it.
    fn new(ctx: &Rc<AppContext>, key: &str) -> (Self, adw::NavigationPage) {
        let header = PageHeader::new("");
        header.set_eyebrow("Artist");
        let image = ArtistImage::new(&ctx.covers, CoverSize::Large);
        header.set_leading(Some(&image));
        header.connect_narrow_notify(glib::clone!(
            #[weak]
            image,
            move |header| image.set_pixel_size(CoverSize::header(header.narrow()).pixels())
        ));
        let play = header.add_button(icons::PLAY, "_Play", true);
        play.set_tooltip_text(Some("Play all songs by this artist"));
        let shuffle = header.add_button(icons::SHUFFLE, "_Shuffle", false);
        shuffle.set_tooltip_text(Some("Play all songs by this artist in random order"));
        browse::set_page_menu(&header.add_menu_button(), ctx, ItemKind::Artist);
        let (detail, page) = DetailPage::new("Artist", &header);

        let albums = CollapsibleItems::new(&page, COLLAPSED_ALBUMS, ("album", "albums"));
        let albums_grid = album_grid(&detail, ctx, &albums, AlbumCardDetail::Year, "Albums");
        let albums_section =
            detail.add_section("Albums", &albums_grid, Some(albums.toggle_button()));

        let songs = CollapsibleItems::new(&page, COLLAPSED_SONGS, ("song", "songs"));
        let songs_slot = adw::Bin::new();
        let songs_section = detail.add_section("Songs", &songs_slot, Some(songs.toggle_button()));

        let appearances = CollapsibleItems::new(&page, COLLAPSED_ALBUMS, ("album", "albums"));
        let appearances_grid = album_grid(
            &detail,
            ctx,
            &appearances,
            AlbumCardDetail::Artist,
            "Appears On",
        );
        let appearances_section = detail.add_section(
            "Appears On",
            &appearances_grid,
            Some(appearances.toggle_button()),
        );

        let state = Self {
            ctx: ctx.clone(),
            key: key.to_owned(),
            artist: RefCell::default(),
            detail,
            header,
            image,
            play,
            shuffle,
            albums,
            albums_section,
            songs,
            songs_section,
            songs_slot,
            songs_show_artists: Cell::default(),
            appearances,
            appearances_section,
        };
        (state, page)
    }

    fn connect_signals(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.play.connect_clicked(move |_| {
            if let Some(page) = weak.upgrade() {
                page.ctx.player.play_all(page.song_ids());
            }
        });
        let weak = Rc::downgrade(self);
        self.shuffle.connect_clicked(move |_| {
            if let Some(page) = weak.upgrade() {
                page.ctx.player.shuffle_tracks(page.song_ids());
            }
        });
    }

    fn item(&self) -> Option<BrowseItem> {
        self.artist.borrow().clone().map(BrowseItem::Artist)
    }

    fn song_ids(&self) -> Vec<mp_core::TrackId> {
        track_actions::ids_of(&self.songs.all())
    }

    /// Shows the artist as the library now has them, or a message if they are gone.
    fn refresh(&self) {
        let Some(artist) = self.ctx.store.artist(&self.key) else {
            self.artist.replace(None);
            self.detail.show_missing(
                icons::ARTIST,
                "Artist Unavailable",
                "This artist is no longer in your library. Their files may have been \
                 moved, renamed or deleted.",
            );
            return;
        };
        let discography = Discography::of(&self.ctx.store, &artist);
        self.show_header(&artist, &discography);
        let guests =
            has_guest_artists(discography.tracks.iter().map(TrackObject::track), &self.key);

        self.albums.set(discography.albums);
        self.albums_section.set_visible(!self.albums.is_empty());
        self.songs.set(discography.tracks);
        self.songs_section.set_visible(!self.songs.is_empty());
        self.show_song_list(guests);
        self.appearances.set(discography.appearances);
        self.appearances_section
            .set_visible(!self.appearances.is_empty());

        self.artist.replace(Some(artist));
        self.detail.show_content();
    }

    fn show_header(&self, artist: &ArtistObject, discography: &Discography) {
        let name = artist.name();
        self.detail.set_title(&name);
        self.header.set_title(&name);
        self.image.set_artist(&name, artist.cover());

        let own_duration: u64 = discography
            .albums
            .iter()
            .map(AlbumObject::duration_ms)
            .sum();
        self.header.set_subtitle(&format!(
            "{} · {}",
            artist_stats(artist.album_count() as usize, artist.track_count() as usize),
            total_length(own_duration)
        ));
        let playable = !discography.tracks.is_empty();
        self.play.set_sensitive(playable);
        self.shuffle.set_sensitive(playable);
    }

    /// Builds the song list, showing each song's artist only when some are by others
    /// (featured artists), as the rest would repeat the page's title.
    fn show_song_list(&self, artists: bool) {
        if self.songs_show_artists.replace(Some(artists)) != Some(artists) {
            self.songs_slot
                .set_child(Some(&song_list(&self.ctx, &self.songs, artists)));
        }
    }
}

/// A grid of albums that grows with `page` instead of scrolling.
fn album_grid(
    page: &DetailPage,
    ctx: &Rc<AppContext>,
    albums: &CollapsibleItems<AlbumObject>,
    detail: AlbumCardDetail,
    label: &str,
) -> CardGrid {
    let grid = CardGrid::builder(ctx, CardKind::Albums(detail))
        .scrolls(false)
        .build(albums.model());
    grid.grid_view()
        .update_property(&[gtk::accessible::Property::Label(label)]);
    page.narrow_breakpoint()
        .add_setter(&grid, "narrow", Some(&true.to_value()));
    grid
}

/// The Songs section's list. Activating a row plays every song from there, listed
/// or not.
fn song_list(
    ctx: &Rc<AppContext>,
    songs: &Rc<CollapsibleItems<TrackObject>>,
    artists: bool,
) -> TrackList {
    let weak_ctx = Rc::downgrade(ctx);
    let weak_songs = Rc::downgrade(songs);
    TrackList::builder(ctx)
        .columns(&[
            TrackColumn::Title,
            TrackColumn::Album,
            TrackColumn::Duration,
        ])
        .covers(true)
        .artists(artists)
        .sortable(false)
        .scrolls(false)
        .on_activate(move |_, position| {
            if let (Some(ctx), Some(songs)) = (weak_ctx.upgrade(), weak_songs.upgrade()) {
                let ids = track_actions::ids_of(&songs.all());
                ctx.player.play_tracks(ids, position as usize);
            }
        })
        .build(songs.model())
}
