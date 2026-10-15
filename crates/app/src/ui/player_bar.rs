//! The now-playing bar along the bottom of the window: what's playing on the left,
//! transport controls and the seek bar in the middle, queue and volume on the right.
//!
//! Dragging its top edge makes the visualiser taller or shorter, and off when dragged
//! low enough. The visualiser only takes room while music plays: it unfolds when
//! playback starts and folds away shortly after it stops.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;
use mp_core::settings::WindowState;

use crate::context::AppContext;
use crate::model::TrackObject;
use crate::player::{Player, PlayerState};
use crate::ui::navigation::Route;
use crate::ui::widgets::{
    BalancedRow, CoverImage, CoverSize, FormatBadge, Grows, Resizable, SeekBar, TransportControls,
    Visualizer, VolumeControls, resize_handle,
};
use crate::ui::{ellipsized_label, icon_toggle, icons, link_button, set_css_class};

/// The widest the transport controls and seek bar grow.
const CONTROLS_MAX_WIDTH: i32 = 620;
/// Up to this width, a long title or quality details on the left are cut short rather
/// than squeezing the seek bar or pushing it off centre.
const CONTROLS_WIDTH: i32 = 520;
/// Heights, in pixels, the visualiser can be dragged to.
const VISUALIZER_MIN_HEIGHT: i32 = 16;
const VISUALIZER_MAX_HEIGHT: i32 = 160;
/// Dragged shorter than this, the visualiser turns off.
const VISUALIZER_OFF_BELOW: f64 = 10.0;
/// How long the visualiser stays after the music stops, while its bars fall, before it
/// folds away. Skipping songs is over by then, so it doesn't jump.
const VISUALIZER_FOLD_DELAY: Duration = Duration::from_millis(1200);
const VISUALIZER_FOLD_MS: u32 = 250;

/// The bar's child widgets, created once.
///
/// The compact layout rearranges some of them: the transport buttons move to the end
/// next to the queue button, and the seek slider becomes a thin line along the top.
struct Widgets {
    layout: BalancedRow,
    now: NowPlaying,
    /// The centre column: transport buttons above the seek bar.
    controls: adw::Clamp,
    controls_box: gtk::Box,
    transport: TransportControls,
    seek_bar: SeekBar,
    visualizer: Visualizer,
    /// Holds the seek slider in the compact layout.
    top_slot: adw::Bin,
    extras: Extras,
}

/// What's playing: the cover, which opens Now Playing, and the title and artist,
/// which open their pages. The format badge follows the artist, or with full quality
/// details, starts a line of its own before them.
struct NowPlaying {
    root: gtk::Box,
    cover: CoverImage,
    cover_button: gtk::Button,
    title: gtk::Label,
    title_button: gtk::Button,
    artist: gtk::Label,
    artist_button: gtk::Button,
    artist_row: gtk::Box,
    badge: FormatBadge,
    quality_row: gtk::Box,
    details: gtk::Label,
}

/// The queue toggle and the volume controls at the end of the bar.
struct Extras {
    root: gtk::Box,
    queue: gtk::ToggleButton,
    volume: VolumeControls,
}

mod imp {
    use super::*;

    #[derive(Default, glib::Properties)]
    #[properties(wrapper_type = super::PlayerBar)]
    pub struct PlayerBar {
        /// Narrow layout: hides the volume slider and secondary controls.
        #[property(get, set = Self::set_compact, explicit_notify)]
        pub(super) compact: Cell<bool>,
        /// Whether the queue panel is open; bound to the window's split view.
        #[property(get, set)]
        pub(super) queue_visible: Cell<bool>,
        pub(super) ctx: OnceCell<Rc<AppContext>>,
        pub(super) widgets: OnceCell<Widgets>,
        pub(super) visualizer_height: Cell<i32>,
        /// How far the visualiser is unfolded, from 0 (taking no room) to 1.
        pub(super) visualizer_reveal: Cell<f64>,
        pub(super) fold_animation: OnceCell<adw::TimedAnimation>,
        pub(super) fold_timeout: RefCell<Option<glib::SourceId>>,
        pub(super) playing: Cell<bool>,
        /// The top edge is being dragged, which shows the visualiser even without music.
        pub(super) resizing: Cell<bool>,
        pub(super) resize_handle: OnceCell<gtk::Widget>,
    }

    impl PlayerBar {
        fn set_compact(&self, compact: bool) {
            if self.compact.replace(compact) != compact {
                self.obj().apply_compact();
                self.obj().notify_compact();
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PlayerBar {
        const NAME: &'static str = "MpPlayerBar";
        type Type = super::PlayerBar;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("playerbar");
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for PlayerBar {}
    impl WidgetImpl for PlayerBar {}
    impl BinImpl for PlayerBar {}
}

glib::wrapper! {
    /// The now-playing bar (`mp-player-bar`).
    pub struct PlayerBar(ObjectSubclass<imp::PlayerBar>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl PlayerBar {
    pub fn new(ctx: &Rc<AppContext>) -> Self {
        let bar: Self = glib::Object::new();
        bar.add_css_class("mp-player-bar");
        let imp = bar.imp();
        assert!(imp.ctx.set(ctx.clone()).is_ok(), "a new bar has no context");

        let (layout, widgets) = build_layout(ctx);
        let overlay = gtk::Overlay::builder().child(&layout).build();
        let handle = resize_handle(Grows::Up, bar.downgrade());
        overlay.add_overlay(&handle);
        bar.set_child(Some(&overlay));
        assert!(imp.widgets.set(widgets).is_ok(), "a new bar has no widgets");
        assert!(
            imp.resize_handle.set(handle).is_ok(),
            "a new bar has no handle"
        );
        let unfold = adw::CallbackAnimationTarget::new(glib::clone!(
            #[weak]
            bar,
            move |reveal| bar.set_visualizer_reveal(reveal)
        ));
        let animation = adw::TimedAnimation::builder()
            .widget(&bar)
            .duration(VISUALIZER_FOLD_MS)
            .easing(adw::Easing::EaseOutCubic)
            .target(&unfold)
            .build();
        assert!(
            imp.fold_animation.set(animation).is_ok(),
            "a new bar has no animation"
        );
        bar.set_visualizer_reveal(0.0);
        bar.set_visualizer_height(ctx.settings().window.visualizer_height);

        bar.bind_property("queue-visible", &bar.widgets().extras.queue, "active")
            .bidirectional()
            .sync_create()
            .build();
        bar.connect_now_playing_links();
        bar.follow_player();
        bar.update_track();
        bar.update_state();
        bar
    }

    fn ctx(&self) -> &Rc<AppContext> {
        self.imp().ctx.get().expect("created in PlayerBar::new")
    }

    fn player(&self) -> &Player {
        &self.ctx().player
    }

    fn widgets(&self) -> &Widgets {
        self.imp().widgets.get().expect("created in PlayerBar::new")
    }

    /// The cover opens Now Playing, the title the current track's album and the
    /// artist its artist.
    fn connect_now_playing_links(&self) {
        let ctx = Rc::downgrade(self.ctx());
        let go_to = move |route: fn(&TrackObject) -> Route| {
            let ctx = ctx.clone();
            move |_: &gtk::Button| {
                if let Some(ctx) = ctx.upgrade()
                    && let Some(track) = ctx.player.current_track()
                {
                    ctx.navigator.navigate(route(&track));
                }
            }
        };
        let now = &self.widgets().now;
        now.cover_button
            .connect_clicked(go_to(|_| Route::NowPlaying));
        now.title_button
            .connect_clicked(go_to(|track| Route::Album(track.album_key())));
        now.artist_button
            .connect_clicked(go_to(|track| Route::Artist(track.artist_key())));
    }

    /// Keeps the bar in step with the player and the display options.
    fn follow_player(&self) {
        let player = self.player();
        let weak = self.downgrade();
        let on = move |update: fn(&PlayerBar)| {
            let weak = weak.clone();
            move |_: &Player| {
                if let Some(bar) = weak.upgrade() {
                    update(&bar);
                }
            }
        };
        player.connect_current_track_notify(on(PlayerBar::update_track));
        player.connect_state_notify(on(PlayerBar::update_state));

        let display = &self.ctx().display;
        display.connect_visualizer_notify(glib::clone!(
            #[weak(rename_to = bar)]
            self,
            move |_| bar.update_visualizer_visibility()
        ));
        display.connect_quality_details_notify(glib::clone!(
            #[weak(rename_to = bar)]
            self,
            move |_| bar.update_quality()
        ));
    }

    fn update_track(&self) {
        let w = self.widgets();
        let track = self.player().current_track();
        let has_track = track.is_some();
        match &track {
            Some(track) => {
                w.now.title.set_label(&track.title());
                w.now.artist.set_label(&track.artist());
                w.now.cover.set_cover(track.track().cover.as_ref());
                w.now
                    .title_button
                    .set_tooltip_text(Some(&format!("Go to {}", track.album())));
                w.now
                    .artist_button
                    .set_tooltip_text(Some(&format!("Go to {}", track.album_artist())));
            }
            None => {
                w.now.title.set_label("Not Playing");
                w.now.artist.set_label("");
                w.now.cover.set_cover(None);
            }
        }
        w.now.badge.set_track(track.as_ref().map(|t| t.track()));
        let details = track
            .as_ref()
            .map(|t| t.track().quality_details())
            .unwrap_or_default();
        w.now.details.set_label(&details);
        w.now.details.set_tooltip_text(Some(&details));
        self.update_quality();
        w.now.artist_button.set_visible(has_track);
        w.now.title_button.set_sensitive(has_track);
        w.now.cover_button.set_sensitive(has_track);
        set_css_class(&w.now.title, "dim-label", !has_track);
    }

    fn update_state(&self) {
        let playing = self.player().state() == PlayerState::Playing;
        self.widgets().visualizer.set_playing(playing);
        if self.imp().playing.replace(playing) != playing {
            self.update_visualizer_visibility();
        }
    }

    fn set_visualizer_height(&self, height: i32) {
        let height = height.clamp(VISUALIZER_MIN_HEIGHT, VISUALIZER_MAX_HEIGHT);
        self.imp().visualizer_height.set(height);
        self.apply_visualizer_size();
    }

    /// Unfolds the visualiser while music plays (or the bar's edge is dragged), and folds
    /// it away a moment after the music stops. Switched off, or in the compact layout,
    /// it goes at once.
    fn update_visualizer_visibility(&self) {
        let imp = self.imp();
        if let Some(source) = imp.fold_timeout.take() {
            source.remove();
        }
        let allowed = self.ctx().display.visualizer() && !self.compact();
        if !allowed || imp.resizing.get() {
            self.reveal_visualizer_now(if allowed { 1.0 } else { 0.0 });
        } else if imp.playing.get() {
            self.animate_visualizer_to(1.0);
        } else if imp.visualizer_reveal.get() > 0.0 {
            let bar = self.downgrade();
            let source = glib::timeout_add_local_once(VISUALIZER_FOLD_DELAY, move || {
                if let Some(bar) = bar.upgrade() {
                    bar.imp().fold_timeout.take();
                    bar.animate_visualizer_to(0.0);
                }
            });
            imp.fold_timeout.replace(Some(source));
        }
    }

    fn animate_visualizer_to(&self, reveal: f64) {
        let imp = self.imp();
        let Some(animation) = imp.fold_animation.get() else {
            return;
        };
        let running = animation.state() == adw::AnimationState::Playing;
        if running && animation.value_to() == reveal {
            return;
        }
        animation.pause();
        let current = imp.visualizer_reveal.get();
        if current != reveal {
            animation.set_value_from(current);
            animation.set_value_to(reveal);
            animation.play();
        }
    }

    fn reveal_visualizer_now(&self, reveal: f64) {
        if let Some(animation) = self.imp().fold_animation.get() {
            animation.pause();
        }
        self.set_visualizer_reveal(reveal);
    }

    fn set_visualizer_reveal(&self, reveal: f64) {
        self.imp().visualizer_reveal.set(reveal);
        self.apply_visualizer_size();
    }

    /// Sizes the visualiser for its height and how far it is unfolded.
    fn apply_visualizer_size(&self) {
        let imp = self.imp();
        let Some(w) = imp.widgets.get() else { return };
        let visualizer = &w.visualizer;
        let reveal = imp.visualizer_reveal.get();
        visualizer.set_visible(reveal > 0.0);
        if reveal <= 0.0 {
            return;
        }
        // Its CSS margins overlap the seek bar, so it takes no room at all while its
        // height is that overlap; it unfolds from there.
        let height = imp.visualizer_height.get();
        visualizer.set_bar_height(height);
        let (room, ..) = visualizer.measure(gtk::Orientation::Vertical, -1);
        let folded = (height - room).max(0);
        let unfolded = (f64::from(height - folded) * reveal).round() as i32;
        visualizer.set_bar_height(folded + unfolded);
    }

    /// Places and shows the format badge and quality details for the current layout.
    /// Narrow windows have room for neither.
    fn update_quality(&self) {
        let Some(w) = self.imp().widgets.get() else {
            return;
        };
        let now = &w.now;
        let full = self.ctx().display.quality_details();
        let row = if full {
            &now.quality_row
        } else {
            &now.artist_row
        };
        if now.badge.parent().as_ref() != Some(row.upcast_ref()) {
            if let Some(old) = now.badge.parent().and_downcast::<gtk::Box>() {
                old.remove(&now.badge);
            }
            if full {
                row.prepend(&now.badge);
            } else {
                row.append(&now.badge);
            }
        }
        let shown = !self.compact() && self.player().current_track().is_some();
        now.badge.set_visible(shown);
        now.quality_row.set_visible(shown && full);
        now.details.set_visible(!now.details.label().is_empty());
    }

    /// Switches between the full layout and the compact one for narrow windows.
    fn apply_compact(&self) {
        let Some(w) = self.imp().widgets.get() else {
            return;
        };
        let compact = self.compact();
        if compact {
            w.layout.set_center_widget(None::<&gtk::Widget>);
            w.controls_box.remove(&w.transport);
            w.extras.root.prepend(&w.transport);
            w.top_slot.set_child(Some(&w.seek_bar.take_slider()));
        } else {
            w.top_slot.set_child(None::<&gtk::Widget>);
            w.seek_bar.restore_slider();
            w.extras.root.remove(&w.transport);
            w.controls_box.prepend(&w.transport);
            w.layout.set_center_widget(Some(&w.controls));
        }
        w.top_slot.set_visible(compact);
        w.transport.set_show_modes(!compact);
        w.extras.volume.set_visible(!compact);
        self.update_quality();
        set_css_class(self, "mp-compact", compact);
        if let Some(handle) = self.imp().resize_handle.get() {
            handle.set_visible(!compact);
        }
        self.update_visualizer_visibility();
    }
}

/// The bar's top edge, which sets the visualiser's height.
impl Resizable for glib::WeakRef<PlayerBar> {
    fn size(&self) -> f64 {
        let Some(bar) = self.upgrade() else {
            return 0.0;
        };
        let imp = bar.imp();
        if bar.ctx().display.visualizer() && imp.visualizer_reveal.get() > 0.0 {
            f64::from(imp.visualizer_height.get())
        } else {
            0.0
        }
    }

    fn resize(&self, size: f64) {
        let Some(bar) = self.upgrade() else { return };
        bar.imp().resizing.set(true);
        let enabled = size >= VISUALIZER_OFF_BELOW;
        if enabled {
            bar.set_visualizer_height(size.round() as i32);
        }
        if bar.ctx().display.visualizer() != enabled {
            bar.ctx().set_visualizer(enabled);
        }
    }

    /// Remembers the height. Turned off, the visualiser comes back at its usual height
    /// when turned on again. Without music it folds away again.
    fn resized(&self) {
        let Some(bar) = self.upgrade() else { return };
        bar.imp().resizing.set(false);
        bar.update_visualizer_visibility();
        if !bar.ctx().display.visualizer() {
            bar.set_visualizer_height(WindowState::default().visualizer_height);
        }
        let height = bar.imp().visualizer_height.get();
        bar.ctx()
            .update_settings(|settings| settings.window.visualizer_height = height);
    }
}

fn build_layout(ctx: &Rc<AppContext>) -> (gtk::Box, Widgets) {
    let now = now_playing(ctx);
    let transport = TransportControls::new(ctx);
    let seek_bar = SeekBar::new(ctx);
    let visualizer = Visualizer::new(ctx.player.scope());
    seek_bar.set_above(&visualizer);
    let extras = extras(ctx);

    let controls_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .valign(gtk::Align::Center)
        .build();
    controls_box.append(&transport);
    controls_box.append(&seek_bar);
    let controls = adw::Clamp::builder()
        .maximum_size(CONTROLS_MAX_WIDTH)
        .tightening_threshold(420)
        .child(&controls_box)
        .css_classes(["mp-controls"])
        .build();

    let layout = BalancedRow::new(CONTROLS_WIDTH);
    layout.add_css_class("mp-player-bar-content");
    layout.set_start_widget(Some(&now.root));
    layout.set_center_widget(Some(&controls));
    layout.set_end_widget(Some(&extras.root));
    let top_slot = adw::Bin::builder()
        .visible(false)
        .css_classes(["mp-player-bar-progress"])
        .build();
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.append(&top_slot);
    root.append(&layout);

    let widgets = Widgets {
        layout,
        now,
        controls,
        controls_box,
        transport,
        seek_bar,
        visualizer,
        top_slot,
        extras,
    };
    (root, widgets)
}

fn now_playing(ctx: &AppContext) -> NowPlaying {
    let cover = CoverImage::new(&ctx.covers, CoverSize::Small);
    let cover_button = gtk::Button::builder()
        .child(&cover)
        .css_classes(["flat", "mp-cover-button"])
        .tooltip_text("Show Now Playing")
        .valign(gtk::Align::Center)
        .build();
    let title = ellipsized_label(&["mp-now-title", "heading"]);
    let title_button = link_button(&title);
    let artist = ellipsized_label(&["mp-now-artist", "dim-label"]);
    let artist_button = link_button(&artist);
    let badge = FormatBadge::new();
    badge.add_css_class("mp-now-quality");

    let details = ellipsized_label(&["mp-now-quality-details", "caption", "dim-label", "numeric"]);

    let artist_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    artist_row.append(&artist_button);
    artist_row.append(&badge);
    let quality_row = gtk::Box::builder()
        .spacing(6)
        .visible(false)
        .css_classes(["mp-now-quality-row"])
        .build();
    quality_row.append(&details);
    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .valign(gtk::Align::Center)
        .spacing(2)
        .build();
    text.append(&title_button);
    text.append(&artist_row);
    text.append(&quality_row);
    let root = gtk::Box::builder()
        .spacing(12)
        .css_classes(["mp-now-playing"])
        .build();
    root.append(&cover_button);
    root.append(&text);
    NowPlaying {
        root,
        cover,
        cover_button,
        title,
        title_button,
        artist,
        artist_button,
        artist_row,
        badge,
        quality_row,
        details,
    }
}

fn extras(ctx: &Rc<AppContext>) -> Extras {
    let queue = icon_toggle(
        icons::QUEUE,
        "Queue",
        &["flat", "circular", "mp-toggle-button"],
    );
    let volume = VolumeControls::new(ctx);
    let root = gtk::Box::builder()
        .spacing(4)
        .halign(gtk::Align::End)
        .valign(gtk::Align::Center)
        .css_classes(["mp-player-extras"])
        .build();
    root.append(&queue);
    root.append(&volume);
    Extras {
        root,
        queue,
        volume,
    }
}
