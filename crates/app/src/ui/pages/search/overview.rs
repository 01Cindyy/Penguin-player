//! The "All" view of the search page: the top result beside the first songs, then
//! shelves of albums and artists.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

use super::ALBUM_CARDS;
use super::results::SearchResults;
use crate::context::AppContext;
use crate::model::TrackObject;
use crate::ui::widgets::SectionHeader;
use crate::ui::widgets::browse::{CardKind, CardShelf};
use crate::ui::widgets::discover::{SongList, TopResultCard};

/// Songs listed under the top result; the rest are a "Show All" away.
const PREVIEW_SONGS: usize = 8;
/// Songs listed beside the top result: about as tall as its card.
const PREVIEW_SONGS_BESIDE: usize = 5;
/// Cards prepared for each overview shelf; as many as fit are shown.
const SHELF_CARDS: usize = 12;

/// The "All" view: the top result beside the first songs, then shelves of albums
/// and artists.
pub(super) struct Overview {
    pub(super) scrolled: gtk::ScrolledWindow,
    pub(super) content: gtk::Box,
    pub(super) top_row: gtk::Box,
    top_section: gtk::Box,
    pub(super) top: TopResultCard,
    pub(super) songs_header: SectionHeader,
    songs_section: gtk::Box,
    pub(super) song_list: SongList,
    songs: gio::ListStore,
    pub(super) albums: ShelfSection,
    pub(super) artists: ShelfSection,
}

impl Overview {
    pub(super) fn new(ctx: &Rc<AppContext>) -> Self {
        let top = TopResultCard::new(&ctx.covers);
        top.set_vexpand(true);
        let top_section = section(
            &SectionHeader::new("Top Result", ("result", "results")),
            &top,
        );
        top_section.add_css_class("mp-top-result-section");
        // The card's labels expand, which would otherwise make it share the row's
        // spare width with the songs.
        top_section.set_hexpand(false);

        let songs = gio::ListStore::new::<TrackObject>();
        let song_list = SongList::new(ctx, &songs);
        let songs_header = SectionHeader::new("Songs", ("song", "songs"));
        let songs_section = section(&songs_header, &song_list);
        songs_section.set_hexpand(true);

        let top_row = gtk::Box::builder()
            .spacing(24)
            .css_classes(["mp-search-top-row"])
            .build();
        top_row.append(&top_section);
        top_row.append(&songs_section);

        let albums = ShelfSection::new(ctx, "Albums", ALBUM_CARDS);
        let artists = ShelfSection::new(ctx, "Artists", CardKind::Artists);

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(30)
            .css_classes(["mp-search-overview"])
            .build();
        content.append(&top_row);
        content.append(&albums.root);
        content.append(&artists.root);
        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&content)
            .build();

        Self {
            scrolled,
            content,
            top_row,
            top_section,
            top,
            songs_header,
            songs_section,
            song_list,
            songs,
            albums,
            artists,
        }
    }

    pub(super) fn show(&self, results: &SearchResults) {
        if let Some(top) = &results.top {
            self.top.set_result(top);
        }
        self.top_section.set_visible(results.top.is_some());
        let preview_len = match self.top_row.orientation() {
            gtk::Orientation::Horizontal => PREVIEW_SONGS_BESIDE,
            _ => PREVIEW_SONGS,
        };
        let preview: Vec<TrackObject> = results.songs.iter().take(preview_len).cloned().collect();
        self.songs.splice(0, self.songs.n_items(), &preview);
        self.songs_section.set_visible(!preview.is_empty());
        self.songs_header
            .set_show_all(results.songs.len(), preview.len());
        self.albums.show(&results.albums[..]);
        self.artists.show(&results.artists[..]);
    }

    pub(super) fn scroll_to_top(&self) {
        self.scrolled.vadjustment().set_value(0.0);
    }
}

/// A titled shelf of album or artist cards with a "Show All" button.
pub(super) struct ShelfSection {
    root: gtk::Box,
    pub(super) header: SectionHeader,
    shelf: CardShelf,
    /// How many results the shelf stands for, of which it shows as many as fit.
    total: Rc<Cell<usize>>,
}

impl ShelfSection {
    fn new(ctx: &Rc<AppContext>, title: &str, kind: CardKind) -> Self {
        let noun = match kind {
            CardKind::Albums(_) => ("album", "albums"),
            CardKind::Artists => ("artist", "artists"),
        };
        let header = SectionHeader::new(title, noun);
        let shelf = CardShelf::new(ctx, kind);
        let root = section(&header, &shelf);
        let total = Rc::new(Cell::new(0));
        let shown_header = header.clone();
        let shown_total = total.clone();
        shelf.connect_shown_notify(move |shelf| {
            shown_header.set_show_all(shown_total.get(), shelf.shown() as usize);
        });
        Self {
            root,
            header,
            shelf,
            total,
        }
    }

    fn show(&self, results: &[impl IsA<glib::Object>]) {
        let cards = &results[..results.len().min(SHELF_CARDS)];
        self.total.set(results.len());
        self.shelf.set_items(cards);
        self.header
            .set_show_all(results.len(), self.shelf.shown() as usize);
        self.root.set_visible(!cards.is_empty());
    }
}

/// A heading with `content` underneath, hidden until there are results to show.
fn section(header: &SectionHeader, content: &impl IsA<gtk::Widget>) -> gtk::Box {
    let section = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .visible(false)
        .css_classes(["mp-search-section"])
        .build();
    section.append(&header.root);
    section.append(content);
    section
}
