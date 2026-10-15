//! Search results that update as you type: the best match and the first songs, then
//! albums and artists. Chips switch between that overview and the full list of one
//! kind.
//!
//! The page is built when a search starts. Later queries arrive through
//! [`Navigator::connect_search_changed`](crate::ui::navigation::Navigator::connect_search_changed);
//! the search field already waits for a pause in typing before sending one. Enter in
//! the field plays the top result.

mod full_lists;
mod overview;
mod results;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use mp_core::TrackId;

use crate::context::AppContext;
use crate::model::TrackObject;
use crate::ui::icons;
use crate::ui::navigation::Route;
use crate::ui::pages::NARROW_PAGE_WIDTH;
use crate::ui::widgets::PageHeader;
use crate::ui::widgets::browse::{AlbumCardDetail, CardKind};
use crate::ui::widgets::discover::TopResult;
use crate::ui::{disconnect_on_destroy, keep_alive, mark_narrow_while, max_width_breakpoint};
use crate::util::count_label;

use full_lists::FullLists;
use overview::Overview;
use results::{Query, SearchResults, Searchable};

/// Albums show their artist under the title, as on the Albums page.
const ALBUM_CARDS: CardKind = CardKind::Albums(AlbumCardDetail::Artist);
/// Below this width (in sp) the top result sits above the songs instead of beside them.
const STACKED_WIDTH: f64 = 780.0;

/// What the page shows: the overview, or every result of one kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    All,
    Songs,
    Albums,
    Artists,
}

impl View {
    const ALL: [View; 4] = [View::All, View::Songs, View::Albums, View::Artists];

    fn label(self) -> &'static str {
        match self {
            View::All => "All",
            View::Songs => "Songs",
            View::Albums => "Albums",
            View::Artists => "Artists",
        }
    }

    /// The view's page in the results stack.
    fn stack_name(self) -> &'static str {
        match self {
            View::All => "all",
            View::Songs => "songs",
            View::Albums => "albums",
            View::Artists => "artists",
        }
    }

    fn count(self, results: &SearchResults) -> usize {
        match self {
            View::All => results.songs.len() + results.albums.len() + results.artists.len(),
            View::Songs => results.songs.len(),
            View::Albums => results.albums.len(),
            View::Artists => results.artists.len(),
        }
    }
}

pub fn build(ctx: &Rc<AppContext>, query: &str) -> adw::NavigationPage {
    let page = SearchPage::new(ctx);
    page.connect_signals();
    page.set_query(query);

    let navigation_page = super::page_with_header("Search", &page.header, &page.body);
    navigation_page.add_css_class("mp-search-page");

    let weak = Rc::downgrade(&page);
    let handler = ctx.store.connect_tracks_changed(move |_| {
        if let Some(page) = weak.upgrade() {
            page.library_changed();
        }
    });
    disconnect_on_destroy(&navigation_page, &ctx.store, handler);

    let weak = Rc::downgrade(&page);
    let handler = ctx.navigator.connect_search_changed(move |query| {
        if let Some(page) = weak.upgrade() {
            page.set_query(query);
        }
    });
    disconnect_on_destroy(&navigation_page, &ctx.navigator, handler);
    let weak = Rc::downgrade(&page);
    let handler = ctx.navigator.connect_search_activated(move |query| {
        if let Some(page) = weak.upgrade() {
            page.set_query(query);
            page.play_top_result();
        }
    });
    disconnect_on_destroy(&navigation_page, &ctx.navigator, handler);

    keep_alive(&navigation_page, page);
    navigation_page
}

/// The page's state, owned by its content widget (see [`keep_alive`]).
struct SearchPage {
    ctx: Rc<AppContext>,
    /// The query as typed (trimmed), for the title.
    typed_query: RefCell<String>,
    library: RefCell<Searchable>,
    results: RefCell<SearchResults>,
    view: Cell<View>,
    /// Views that still show an older search; each is refilled when next shown.
    stale: Cell<[bool; 4]>,
    header: PageHeader,
    play: gtk::Button,
    shuffle: gtk::Button,
    chips: Vec<(View, gtk::ToggleButton)>,
    chip_box: adw::WrapBox,
    body: adw::BreakpointBin,
    stack: gtk::Stack,
    status: adw::StatusPage,
    overview: Overview,
    lists: FullLists,
}

impl SearchPage {
    fn new(ctx: &Rc<AppContext>) -> Rc<Self> {
        let header = PageHeader::new("Search");
        header.set_eyebrow("Search");
        let play = header.add_button(icons::PLAY, "_Play", true);
        play.set_tooltip_text(Some("Play the matching songs"));
        let shuffle = header.add_button(icons::SHUFFLE, "_Shuffle", false);
        shuffle.set_tooltip_text(Some("Play the matching songs in random order"));
        let (chip_box, chips) = view_chips();
        header.set_extra(&chip_box);

        let status = adw::StatusPage::builder()
            .icon_name(icons::SEARCH)
            .css_classes(["compact", "mp-search-status"])
            .build();
        let overview = Overview::new(ctx);
        let lists = FullLists::new(ctx);
        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .transition_duration(120)
            .vexpand(true)
            .build();
        stack.add_named(&status, Some("status"));
        stack.add_named(&overview.scrolled, Some(View::All.stack_name()));
        stack.add_named(&lists.songs, Some(View::Songs.stack_name()));
        stack.add_named(&lists.albums, Some(View::Albums.stack_name()));
        stack.add_named(&lists.artists, Some(View::Artists.stack_name()));

        // A breakpoint bin needs a minimum size of its own; the page has room for it.
        let body = adw::BreakpointBin::builder()
            .width_request(240)
            .height_request(160)
            .vexpand(true)
            .child(&stack)
            .build();

        let page = Rc::new(Self {
            ctx: ctx.clone(),
            typed_query: RefCell::default(),
            library: RefCell::new(Searchable::new(&ctx.store)),
            results: RefCell::default(),
            view: Cell::new(View::All),
            stale: Cell::new([false; 4]),
            header,
            play,
            shuffle,
            chips,
            chip_box,
            body,
            stack,
            status,
            overview,
            lists,
        });
        page.add_breakpoints();
        page
    }

    fn connect_signals(self: &Rc<Self>) {
        self.connect_view_switching();
        self.connect_playback();
    }

    /// The chips and "Show All" buttons switch views; a layout change refills the
    /// overview.
    fn connect_view_switching(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.overview.top_row.connect_orientation_notify(move |_| {
            let weak = weak.clone();
            // Breakpoints switch the layout while sizes are being allocated; refill
            // the overview afterwards.
            glib::idle_add_local_once(move || {
                if let Some(page) = weak.upgrade() {
                    page.refill_overview();
                }
            });
        });
        for (view, chip) in &self.chips {
            let view = *view;
            let weak = Rc::downgrade(self);
            chip.connect_toggled(move |chip| {
                if chip.is_active()
                    && let Some(page) = weak.upgrade()
                {
                    page.show_view(view);
                }
            });
        }
        for (button, view) in [
            (&self.overview.songs_header.show_all, View::Songs),
            (&self.overview.albums.header.show_all, View::Albums),
            (&self.overview.artists.header.show_all, View::Artists),
        ] {
            let weak = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                if let Some(page) = weak.upgrade() {
                    page.select_view(view);
                }
            });
        }
    }

    /// The header's play buttons, the overview's songs and the top result.
    fn connect_playback(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.overview.song_list.set_on_activate(move |track| {
            if let Some(page) = weak.upgrade() {
                page.play_song(track);
            }
        });

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

        let weak = Rc::downgrade(self);
        self.overview.top.connect_open(move |card| {
            if let (Some(page), Some(result)) = (weak.upgrade(), card.result()) {
                page.open(&result);
            }
        });
        let weak = Rc::downgrade(self);
        self.overview.top.connect_play(move |card| {
            if let (Some(page), Some(result)) = (weak.upgrade(), card.result()) {
                page.play(&result);
            }
        });
    }

    /// Stacks the overview on narrower pages, with the top result spread across the
    /// page. Phones narrow the gutters, and the card goes back to its upright layout
    /// so the title keeps room beside the play button. The full lists of albums and
    /// artists narrow their gutters by themselves.
    fn add_breakpoints(&self) {
        let stacked = max_width_breakpoint(STACKED_WIDTH);
        let narrow = max_width_breakpoint(NARROW_PAGE_WIDTH);
        // Only the last matching breakpoint applies, so the narrow one stacks too.
        for breakpoint in [&stacked, &narrow] {
            breakpoint.add_setter(
                &self.overview.top_row,
                "orientation",
                Some(&gtk::Orientation::Vertical.to_value()),
            );
        }
        stacked.add_setter(&self.overview.top, "horizontal", Some(&true.to_value()));
        mark_narrow_while(&narrow, &self.overview.content);
        self.body.add_breakpoint(stacked);
        self.body.add_breakpoint(narrow);
    }

    fn set_query(&self, typed: &str) {
        self.typed_query.replace(typed.trim().to_owned());
        let query = Query::new(typed);
        if query.text() == self.results.borrow().query.text() && !query.is_empty() {
            // Only case or accents changed: same results, new title.
            self.update_header();
            return;
        }
        let previous = self.results.take();
        let results = SearchResults::find(&self.library.borrow(), query, Some(&previous));
        self.show_results(results);
        self.overview.scroll_to_top();
    }

    /// The library changed: search it again from scratch.
    fn library_changed(&self) {
        self.library.replace(Searchable::new(&self.ctx.store));
        let query = self.results.borrow().query.clone();
        let results = SearchResults::find(&self.library.borrow(), query, None);
        self.show_results(results);
    }

    fn show_results(&self, results: SearchResults) {
        self.results.replace(results);
        self.stale.set([true; 4]);
        self.update_header();
        self.update_chips();
        self.show_view(self.view.get());
    }

    fn update_header(&self) {
        let results = self.results.borrow();
        self.header.set_title(&if results.query.is_empty() {
            "Search".to_owned()
        } else {
            format!("“{}”", self.typed_query.borrow())
        });
        let summary: Vec<String> = [
            (results.songs.len(), "song", "songs"),
            (results.albums.len(), "album", "albums"),
            (results.artists.len(), "artist", "artists"),
        ]
        .into_iter()
        .filter(|(count, ..)| *count > 0)
        .map(|(count, singular, plural)| count_label(count, singular, plural))
        .collect();
        self.header.set_subtitle(&summary.join(" · "));
        let has_songs = !results.songs.is_empty();
        self.play.set_visible(has_songs);
        self.shuffle.set_visible(has_songs);
    }

    /// Shows a chip for each kind that has results, and returns to the overview when
    /// the chosen kind has none left.
    fn update_chips(&self) {
        let results = self.results.borrow();
        self.chip_box.set_visible(!results.is_empty());
        for (view, chip) in &self.chips {
            let count = view.count(&results);
            chip.set_visible(*view == View::All || count > 0);
            chip.set_tooltip_text(Some(&count_label(count, "result", "results")));
        }
        let view = self.view.get();
        let view_emptied = view != View::All && view.count(&results) == 0;
        drop(results);
        if view_emptied {
            self.select_view(View::All);
        }
    }

    /// Shows the overview again, e.g. after its layout changed how many songs fit.
    fn refill_overview(&self) {
        let mut stale = self.stale.get();
        stale[View::All as usize] = true;
        self.stale.set(stale);
        if self.view.get() == View::All {
            self.show_view(View::All);
        }
    }

    fn select_view(&self, view: View) {
        if let Some((_, chip)) = self.chips.iter().find(|(chip_view, _)| *chip_view == view) {
            chip.set_active(true);
        }
        self.show_view(view);
    }

    fn show_view(&self, view: View) {
        self.view.set(view);
        let results = self.results.borrow();
        if results.is_empty() {
            drop(results);
            self.show_status();
            return;
        }
        let mut stale = self.stale.get();
        if std::mem::take(&mut stale[view as usize]) {
            self.stale.set(stale);
            match view {
                View::All => self.overview.show(&results),
                View::Songs => self.lists.show_songs(&results.songs),
                View::Albums => self.lists.show_albums(&results.albums),
                View::Artists => self.lists.show_artists(&results.artists),
            }
        }
        self.stack.set_visible_child_name(view.stack_name());
    }

    /// Explains an empty page: nothing typed yet, no music, or no matches.
    fn show_status(&self) {
        let query_is_empty = self.results.borrow().query.is_empty();
        let (title, description) = if query_is_empty {
            (
                "Search Your Library".to_owned(),
                "Find songs, albums and artists by name, or try a genre or a format \
                 such as FLAC."
                    .to_owned(),
            )
        } else if self.ctx.store.track_count() == 0 {
            (
                "No Music Yet".to_owned(),
                "Songs in your music folder will appear here once they are found.".to_owned(),
            )
        } else {
            (
                "No Results".to_owned(),
                format!(
                    "Nothing in your library matches “{}”. Check the spelling, or try \
                     fewer words.",
                    self.typed_query.borrow()
                ),
            )
        };
        self.status.set_title(&title);
        self.status
            .set_description(Some(&glib::markup_escape_text(&description)));
        self.stack.set_visible_child_name("status");
    }

    fn song_ids(&self) -> Vec<TrackId> {
        let results = self.results.borrow();
        results.songs.iter().map(TrackObject::track_id).collect()
    }

    /// Enter in the search field.
    fn play_top_result(&self) {
        let top = self.results.borrow().top.clone();
        if let Some(top) = top {
            self.play(&top);
        }
    }

    /// Activating the top result card: songs play, albums and artists open.
    fn open(&self, result: &TopResult) {
        let navigator = &self.ctx.navigator;
        match result {
            TopResult::Song(_) => self.play(result),
            TopResult::Album(album) => navigator.navigate(Route::Album(album.key().clone())),
            TopResult::Artist(artist) => navigator.navigate(Route::Artist(artist.key().into())),
        }
    }

    /// Plays the matching songs, as the Songs view lists them, from `track` on.
    fn play_song(&self, track: &TrackObject) {
        let ids = self.song_ids();
        match ids.iter().position(|id| *id == track.track_id()) {
            Some(start) => self.ctx.player.play_tracks(ids, start),
            None => self.ctx.player.play_tracks(vec![track.track_id()], 0),
        }
    }

    /// Albums and artists play everything on them; songs play as [`Self::play_song`].
    fn play(&self, result: &TopResult) {
        let player = &self.ctx.player;
        match result {
            TopResult::Song(track) => self.play_song(track),
            TopResult::Album(album) => player.play_tracks(album.track_ids().to_vec(), 0),
            TopResult::Artist(artist) => {
                let store = &self.ctx.store;
                let ids = artist
                    .album_keys()
                    .iter()
                    .filter_map(|key| store.album(key))
                    .flat_map(|album| album.track_ids().to_vec())
                    .collect();
                player.play_tracks(ids, 0);
            }
        }
    }
}

/// The All / Songs / Albums / Artists chips, in a box for the page header.
fn view_chips() -> (adw::WrapBox, Vec<(View, gtk::ToggleButton)>) {
    let chip_box = adw::WrapBox::builder()
        .child_spacing(6)
        .line_spacing(6)
        .css_classes(["mp-filter-chips"])
        .build();
    let mut group: Option<gtk::ToggleButton> = None;
    let chips = View::ALL
        .into_iter()
        .map(|view| {
            let chip = gtk::ToggleButton::builder()
                .label(view.label())
                .css_classes(["mp-chip"])
                .active(view == View::All)
                .build();
            chip.set_group(group.as_ref());
            group.get_or_insert_with(|| chip.clone());
            chip_box.append(&chip);
            (view, chip)
        })
        .collect();
    (chip_box, chips)
}
