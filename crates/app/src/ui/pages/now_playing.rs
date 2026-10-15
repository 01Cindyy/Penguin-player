//! Now Playing: the playing song filling the window, its art large in the middle over
//! a blur of itself, with the playback controls underneath. While it is shown the
//! window hides the player bar and the search field and narrows the sidebar to its
//! icons (see `MainWindow`).
//!
//! With lyrics on, wide pages show them in their right half, and narrow ones in place
//! of the art.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gio, glib};
use mp_core::{Lyrics, Track};
use mp_plugins::{LyricsSearch, OwnLyrics};

use crate::context::AppContext;
use crate::model::TrackObject;
use crate::player::{Player, PlayerState};
use crate::ui::navigation::Route;
use crate::ui::widgets::{
    CoverBackdrop, CoverImage, FormatBadge, LyricsView, SeekBar, TransportControls, Visualizer,
    VolumeControls,
};
use crate::ui::window::MainWindow;
use crate::ui::{
    disconnect_on_destroy, ellipsized_label, icon_toggle, icons, keep_alive, link_button,
    max_width_breakpoint, set_css_class,
};

/// The largest the art is shown; it shrinks to fit smaller windows.
const COVER_SIZE: i32 = 480;
const VISUALIZER_HEIGHT: i32 = 56;
/// Pages narrower than this (in sp) show lyrics in place of the art, not beside it.
const LYRICS_BESIDE_MIN_WIDTH: f64 = 860.0;
/// Plugins search for a song's lyrics once it has played this long, so skipping
/// through songs doesn't search for each.
const SEARCH_AFTER: Duration = Duration::from_secs(1);
/// Longest wait for plugins to find lyrics.
const SEARCH_TIMEOUT: Duration = Duration::from_secs(20);

pub fn build(ctx: &Rc<AppContext>) -> adw::NavigationPage {
    let page = Rc::new(NowPlayingPage::new(ctx));
    let root = page.layout();
    page.follow(&root);
    page.update_track();
    page.update_state();
    page.update_lyrics();
    page.update_layout();
    keep_alive(&root, page);
    adw::NavigationPage::builder()
        .title("Now Playing")
        .child(&root)
        .build()
}

/// The page's state, owned by its root widget (see [`keep_alive`]).
struct NowPlayingPage {
    ctx: Rc<AppContext>,
    backdrop: CoverBackdrop,
    /// The art, or the lyrics in its place.
    art: gtk::Stack,
    cover: CoverImage,
    title: gtk::Label,
    artist: gtk::Label,
    album: gtk::Label,
    /// Artist and album, or a hint while nothing plays, so the art keeps its size.
    byline: gtk::Box,
    hint: gtk::Label,
    badge: FormatBadge,
    details: gtk::Label,
    visualizer: Visualizer,
    /// The controls, with the lyrics beside them on wide pages.
    split: adw::OverlaySplitView,
    lyrics_toggle: gtk::ToggleButton,
    lyrics_beside: LyricsView,
    lyrics_in_place: LyricsView,
    /// Whether the page is wide enough for the lyrics beside the art.
    wide: Cell<bool>,
    /// The song whose lyrics the views hold, or are loading.
    lyrics_for: RefCell<Option<PathBuf>>,
    /// Counts lyrics loads, so only the latest shows what it finds.
    lyrics_load: Cell<u64>,
    /// What lyrics the song has of its own, once read.
    own_lyrics: Cell<Option<OwnLyrics>>,
    /// The songs plugins find lyrics for, as last seen.
    lyrics_search: Cell<LyricsSearch>,
}

impl NowPlayingPage {
    fn new(ctx: &Rc<AppContext>) -> Self {
        let cover = CoverImage::with_pixel_size(&ctx.covers, COVER_SIZE);
        cover.add_css_class("mp-cover-hero");
        cover.set_can_shrink(true);
        let visualizer = Visualizer::new(ctx.player.scope());
        visualizer.set_bar_height(VISUALIZER_HEIGHT);
        let lyrics_toggle = icon_toggle(
            icons::LYRICS,
            "Lyrics",
            &["flat", "circular", "mp-toggle-button"],
        );
        lyrics_toggle.set_active(ctx.settings().window.now_playing_lyrics);
        Self {
            ctx: ctx.clone(),
            backdrop: CoverBackdrop::new(&ctx.covers),
            art: gtk::Stack::builder()
                .transition_type(gtk::StackTransitionType::Crossfade)
                .transition_duration(250)
                .hhomogeneous(false)
                .vhomogeneous(false)
                .build(),
            cover,
            title: ellipsized_label(&["mp-now-title", "title-2"]),
            artist: ellipsized_label(&["mp-now-artist"]),
            album: ellipsized_label(&["mp-now-album"]),
            byline: gtk::Box::builder()
                .spacing(6)
                .halign(gtk::Align::Center)
                .css_classes(["mp-now-byline"])
                .build(),
            hint: gtk::Label::builder()
                .label("Play something from your library")
                .css_classes(["dim-label"])
                .build(),
            badge: FormatBadge::new(),
            details: ellipsized_label(&[
                "mp-now-quality-details",
                "caption",
                "dim-label",
                "numeric",
            ]),
            visualizer,
            split: adw::OverlaySplitView::builder()
                .sidebar_position(gtk::PackType::End)
                .sidebar_width_fraction(0.5)
                .min_sidebar_width(280.0)
                .max_sidebar_width(f64::from(i32::MAX))
                .show_sidebar(false)
                .enable_show_gesture(false)
                .enable_hide_gesture(false)
                .css_classes(["mp-now-playing-split"])
                .build(),
            lyrics_toggle,
            lyrics_beside: LyricsView::new(ctx),
            lyrics_in_place: LyricsView::new(ctx),
            wide: Cell::new(true),
            lyrics_for: RefCell::default(),
            lyrics_load: Cell::new(0),
            own_lyrics: Cell::new(None),
            lyrics_search: Cell::new(ctx.plugins.lyrics_search()),
        }
    }

    /// The backdrop, a scrim that keeps text readable over it, and the art and
    /// controls in a centred column that scrolls only in very short windows, with the
    /// lyrics beside it on wide pages.
    fn layout(self: &Rc<Self>) -> gtk::Widget {
        let ctx = &self.ctx;
        self.art.add_named(&self.cover, Some("cover"));
        self.art.add_named(&self.lyrics_in_place, Some("lyrics"));
        let art = gtk::AspectFrame::builder()
            .ratio(1.0)
            .obey_child(false)
            .child(&self.art)
            .css_classes(["mp-now-art"])
            .build();

        self.title.set_xalign(0.5);
        let artist = link_button(&self.artist);
        let album = link_button(&self.album);
        artist.connect_clicked(self.go_to(|track| Route::Artist(track.artist_key())));
        album.connect_clicked(self.go_to(|track| Route::Album(track.album_key())));
        let dot = gtk::Label::builder()
            .label("·")
            .css_classes(["dim-label"])
            .build();
        for part in [
            artist.upcast_ref::<gtk::Widget>(),
            dot.upcast_ref(),
            album.upcast_ref(),
            self.hint.upcast_ref(),
        ] {
            self.byline.append(part);
        }

        let quality = gtk::Box::builder()
            .spacing(6)
            .halign(gtk::Align::Center)
            .css_classes(["mp-now-quality-row"])
            .build();
        quality.append(&self.badge);
        quality.append(&self.details);

        let seek_bar = SeekBar::new(ctx);
        seek_bar.set_above(&self.visualizer);
        ctx.display
            .bind_property("visualizer", &self.visualizer, "visible")
            .sync_create()
            .build();

        let queue = icon_toggle(
            icons::QUEUE,
            "Queue",
            &["flat", "circular", "mp-toggle-button"],
        );
        if let Some(window) = ctx.window().and_downcast::<MainWindow>() {
            window.bind_queue_toggle(&queue);
        }
        let page = Rc::downgrade(self);
        self.lyrics_toggle.connect_toggled(move |toggle| {
            if let Some(page) = page.upgrade() {
                let shown = toggle.is_active();
                page.ctx
                    .update_settings(|settings| settings.window.now_playing_lyrics = shown);
                page.update_lyrics();
                page.update_layout();
            }
        });
        let extras = gtk::Box::builder()
            .spacing(4)
            .halign(gtk::Align::Center)
            .css_classes(["mp-player-extras"])
            .build();
        extras.append(&VolumeControls::new(ctx));
        extras.append(&self.lyrics_toggle);
        extras.append(&queue);

        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .valign(gtk::Align::Center)
            .css_classes(["mp-now-playing-column"])
            .build();
        column.append(&art);
        column.append(&self.title);
        column.append(&self.byline);
        column.append(&quality);
        column.append(&seek_bar);
        column.append(&TransportControls::new(ctx));
        column.append(&extras);
        let clamp = adw::Clamp::builder()
            .maximum_size(560)
            .tightening_threshold(400)
            .child(&column)
            .build();
        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&clamp)
            .build();
        let beside = adw::Clamp::builder()
            .maximum_size(560)
            .child(&self.lyrics_beside)
            .css_classes(["mp-now-playing-lyrics"])
            .build();
        self.split.set_content(Some(&scrolled));
        self.split.set_sidebar(Some(&beside));

        let scrim = gtk::Box::builder()
            .can_target(false)
            .css_classes(["mp-now-playing-scrim"])
            .build();
        let overlay = gtk::Overlay::builder()
            .child(&self.backdrop)
            .css_classes(["mp-now-playing-page"])
            .build();
        overlay.add_overlay(&scrim);
        overlay.add_overlay(&self.split);

        // The bin fills the page, so its own minimum size only has to be smaller than
        // the narrowest window.
        let root = adw::BreakpointBin::builder()
            .width_request(300)
            .height_request(200)
            .child(&overlay)
            .build();
        let set_wide = |wide: bool| {
            let page = Rc::downgrade(self);
            move |_: &adw::Breakpoint| {
                if let Some(page) = page.upgrade() {
                    page.wide.set(wide);
                    page.update_layout();
                }
            }
        };
        let narrow = max_width_breakpoint(LYRICS_BESIDE_MIN_WIDTH);
        narrow.connect_apply(set_wide(false));
        narrow.connect_unapply(set_wide(true));
        root.add_breakpoint(narrow);
        root.upcast()
    }

    /// Follows the player and the display options until `root` is destroyed.
    fn follow(self: &Rc<Self>, root: &gtk::Widget) {
        let player = &self.ctx.player;
        let on = |update: fn(&Rc<Self>)| {
            let page = Rc::downgrade(self);
            move |_: &Player| {
                if let Some(page) = page.upgrade() {
                    update(&page);
                }
            }
        };
        for handler in [
            player.connect_current_track_notify(on(|page| {
                page.update_track();
                page.update_lyrics();
                page.update_layout();
            })),
            player.connect_state_notify(on(|page| page.update_state())),
        ] {
            disconnect_on_destroy(root, player, handler);
        }
        let page = Rc::downgrade(self);
        let handler = self.ctx.display.connect_quality_details_notify(move |_| {
            if let Some(page) = page.upgrade() {
                page.update_details();
            }
        });
        disconnect_on_destroy(root, &self.ctx.display, handler);
        // Plugins switched on or off, or set differently, may start or stop looking up
        // the song's lyrics.
        let page = Rc::downgrade(self);
        let handler = self.ctx.plugins.connect_changed(move |plugins| {
            if let Some(page) = page.upgrade() {
                let search = plugins.lyrics_search();
                let before = page.lyrics_search.replace(search);
                if page
                    .own_lyrics
                    .get()
                    .is_some_and(|own| before.covers(own) != search.covers(own))
                {
                    page.reload_lyrics();
                }
            }
        });
        disconnect_on_destroy(root, &self.ctx.plugins, handler);
    }

    /// A click handler that opens `route` for the playing track.
    fn go_to(&self, route: fn(&TrackObject) -> Route) -> impl Fn(&gtk::Button) + 'static {
        let ctx = Rc::downgrade(&self.ctx);
        move |_| {
            if let Some(ctx) = ctx.upgrade()
                && let Some(track) = ctx.player.current_track()
            {
                ctx.navigator.navigate(route(&track));
            }
        }
    }

    fn update_track(&self) {
        let track = self.ctx.player.current_track();
        let cover = track.as_ref().and_then(|track| track.track().cover.clone());
        self.cover.set_cover(cover.as_ref());
        self.backdrop.set_cover(cover.as_ref());
        match &track {
            Some(track) => {
                self.title.set_label(&track.title());
                self.artist.set_label(&track.artist());
                self.album.set_label(&track.album());
            }
            None => self.title.set_label("Not Playing"),
        }
        set_css_class(&self.title, "dim-label", track.is_none());
        let mut part = self.byline.first_child();
        while let Some(widget) = part {
            let is_hint = &widget == self.hint.upcast_ref::<gtk::Widget>();
            widget.set_visible(is_hint == track.is_none());
            part = widget.next_sibling();
        }
        self.badge
            .set_track(track.as_ref().map(|track| track.track()));
        let details = track
            .as_ref()
            .map(|track| track.track().quality_details())
            .unwrap_or_default();
        self.details.set_label(&details);
        self.details.set_tooltip_text(Some(&details));
        self.update_details();
    }

    fn update_details(&self) {
        let shown = self.ctx.display.quality_details() && !self.details.label().is_empty();
        self.details.set_visible(shown);
    }

    fn update_state(&self) {
        let playing = self.ctx.player.state() == PlayerState::Playing;
        self.visualizer.set_playing(playing);
    }

    /// Lyrics show while they are on and a song is playing: beside the art on wide
    /// pages, in its place on narrow ones.
    fn update_layout(&self) {
        let shown = self.lyrics_toggle.is_active() && self.ctx.player.current_track().is_some();
        let wide = self.wide.get();
        self.split.set_show_sidebar(shown && wide);
        self.art
            .set_visible_child_name(if shown && !wide { "lyrics" } else { "cover" });
    }

    /// Loads the playing song's lyrics while they are on, unless they are loaded.
    fn update_lyrics(self: &Rc<Self>) {
        if !self.lyrics_toggle.is_active() {
            return;
        }
        let track = self.playing();
        let path = track.as_ref().map(|track| track.path.clone());
        if *self.lyrics_for.borrow() == path {
            return;
        }
        self.lyrics_for.replace(path);
        self.lyrics_beside.clear();
        self.lyrics_in_place.clear();
        self.load_lyrics(track);
    }

    /// Loads the lyrics again, keeping what is shown until then, or once they are on.
    fn reload_lyrics(self: &Rc<Self>) {
        if self.lyrics_toggle.is_active() {
            self.load_lyrics(self.playing());
        } else {
            self.lyrics_for.replace(None);
        }
    }

    /// Shows `track`'s own lyrics, or those plugins found before if they may replace
    /// them. If plugins look up songs with lyrics like its own, and found none before,
    /// asks them, and shows and keeps what they find.
    fn load_lyrics(self: &Rc<Self>, track: Option<Track>) {
        let load = self.lyrics_load.get() + 1;
        self.lyrics_load.set(load);
        self.own_lyrics.set(None);
        let Some(track) = track else {
            return;
        };
        let page = Rc::downgrade(self);
        let cache = self.ctx.paths.lyrics_dir();
        glib::spawn_future_local(async move {
            let (song, found_dir) = (track.path.clone(), cache.clone());
            let (own, found) = gio::spawn_blocking(move || {
                (
                    mp_library::read_lyrics(&song),
                    mp_library::read_found_lyrics(&found_dir, &song),
                )
            })
            .await
            .unwrap_or_default();
            let Some(this) = page.upgrade().filter(|this| this.is_latest(load)) else {
                return;
            };
            let kind = OwnLyrics::of(own.as_ref());
            this.own_lyrics.set(Some(kind));
            let looked_up = this.ctx.plugins.lyrics_search().covers(kind);
            let found = found.filter(|found| own.is_none() || (looked_up && kind.yield_to(found)));
            if found.is_some() || !looked_up {
                this.show_lyrics(found.or(own), false);
                return;
            }
            let has_own = own.is_some();
            if has_own {
                this.show_lyrics(own, false);
            } else {
                this.lyrics_beside.show_searching();
                this.lyrics_in_place.show_searching();
            }
            let ctx = this.ctx.clone();
            drop(this);

            glib::timeout_future(SEARCH_AFTER).await;
            if !page.upgrade().is_some_and(|this| this.is_latest(load)) {
                return;
            }
            let song = track.path.clone();
            let text =
                glib::future_with_timeout(SEARCH_TIMEOUT, ctx.plugins.find_lyrics(track, kind))
                    .await
                    .ok()
                    .flatten();
            let found = text
                .as_deref()
                .and_then(Lyrics::parse)
                .filter(|found| kind.yield_to(found));
            if let Some(text) = text.filter(|_| found.is_some()) {
                let beside = ctx.settings().save_found_lyrics;
                let song = song.clone();
                gio::spawn_blocking(move || {
                    if let Err(err) = mp_library::save_found_lyrics(&cache, &song, &text, beside) {
                        log::warn!("Could not save the lyrics of {}: {err}", song.display());
                    }
                });
            }
            let Some(this) = page.upgrade().filter(|this| this.is_latest(load)) else {
                return;
            };
            if found.is_some() || !has_own {
                this.show_lyrics(found, true);
            }
        });
    }

    fn playing(&self) -> Option<Track> {
        self.ctx
            .player
            .current_track()
            .map(|track| track.track().clone())
    }

    /// Whether lyrics load `load` is the latest.
    fn is_latest(&self, load: u64) -> bool {
        self.lyrics_load.get() == load
    }

    fn show_lyrics(&self, lyrics: Option<Lyrics>, searched_online: bool) {
        let lyrics = lyrics.map(Rc::new);
        self.lyrics_beside
            .set_lyrics(lyrics.clone(), searched_online);
        self.lyrics_in_place.set_lyrics(lyrics, searched_online);
    }
}
