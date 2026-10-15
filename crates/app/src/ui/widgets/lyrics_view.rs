//! The playing song's lyrics. Timed lyrics follow the song: the line being sung
//! stands out (`mp-lyrics-current`) and is kept in view, and clicking a line jumps
//! there. Scrolling the lyrics by hand stops them following for a few seconds.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk};
use mp_core::{Lyrics, PlayerCommand};

use crate::context::AppContext;
use crate::player::PlayerState;
use crate::ui::{disconnect_on_destroy, follow, icons, set_css_class};

const SCROLL_MS: u32 = 500;
/// How long after the user scrolls the lyrics they follow the song again.
const FOLLOW_AGAIN_AFTER: Duration = Duration::from_secs(4);
/// How far down the view the line being sung is kept, as a fraction of its height.
const CURRENT_LINE_AT: f64 = 0.4;
/// Lines change on time even between the player's position reports, which come
/// about this often.
const POSITION_REPORTS: Duration = Duration::from_millis(250);
/// Height of the faded top and bottom edges, in pixels.
const FADE: f32 = 48.0;
const PAUSE_TEXT: &str = "♪";
const NO_LYRICS: &str = "Lyrics show here when the song has them in its tags, or in an \
                         .lrc file of the same name beside it.";
const NONE_FOUND_ONLINE: &str = "The song has none in its tags or in an .lrc file beside \
                                 it, and plugins found none online.";

struct Parts {
    stack: gtk::Stack,
    scrolled: gtk::ScrolledWindow,
    list: gtk::ListBox,
    missing: adw::StatusPage,
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct LyricsView {
        pub(super) ctx: OnceCell<Rc<AppContext>>,
        pub(super) parts: OnceCell<Parts>,
        pub(super) lyrics: RefCell<Option<Rc<Lyrics>>>,
        /// The line being sung.
        pub(super) current: Cell<Option<usize>>,
        /// The player's last reported position and when it came, to tell the
        /// position between reports.
        pub(super) reported: Cell<(u64, Option<Instant>)>,
        pub(super) next_line: RefCell<Option<glib::SourceId>>,
        /// Set while the user's scrolling has stopped the lyrics following the song.
        pub(super) follow_again: RefCell<Option<glib::SourceId>>,
        /// A scroll to the line being sung that has to wait for the lines' layout.
        pub(super) scroll_pending: Cell<bool>,
        pub(super) scroll: OnceCell<adw::TimedAnimation>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for LyricsView {
        const NAME: &'static str = "MpLyricsView";
        type Type = super::LyricsView;
        type ParentType = adw::Bin;
    }

    impl ObjectImpl for LyricsView {
        fn dispose(&self) {
            for source in [self.next_line.take(), self.follow_again.take()]
                .into_iter()
                .flatten()
            {
                source.remove();
            }
        }
    }

    impl WidgetImpl for LyricsView {
        fn map(&self) {
            self.parent_map();
            self.obj().scroll_to_current(false);
        }

        /// Fades the lines out towards whichever edges have more lines beyond them.
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let Some(parts) = self.parts.get() else {
                return self.parent_snapshot(snapshot);
            };
            let (width, height) = (obj.width() as f32, obj.height() as f32);
            let adjustment = parts.scrolled.vadjustment();
            let above = adjustment.value() as f32;
            let below = (adjustment.upper() - adjustment.page_size()) as f32 - above;
            let (top, bottom) = (above.clamp(0.0, FADE), below.clamp(0.0, FADE));
            if parts.stack.visible_child_name().as_deref() != Some("lines")
                || height <= 2.0 * FADE
                || top + bottom <= 0.0
            {
                return self.parent_snapshot(snapshot);
            }
            let clear = gdk::RGBA::new(1.0, 1.0, 1.0, 0.0);
            let solid = gdk::RGBA::WHITE;
            snapshot.push_mask(gsk::MaskMode::Alpha);
            snapshot.append_linear_gradient(
                &graphene::Rect::new(0.0, 0.0, width, height),
                &graphene::Point::new(0.0, 0.0),
                &graphene::Point::new(0.0, height),
                &[
                    gsk::ColorStop::new(0.0, if top > 0.0 { clear } else { solid }),
                    gsk::ColorStop::new(top / height, solid),
                    gsk::ColorStop::new(1.0 - bottom / height, solid),
                    gsk::ColorStop::new(1.0, if bottom > 0.0 { clear } else { solid }),
                ],
            );
            snapshot.pop();
            self.parent_snapshot(snapshot);
            snapshot.pop();
        }
    }

    impl BinImpl for LyricsView {}
}

glib::wrapper! {
    /// The playing song's lyrics (`mp-lyrics`), or a note that it has none.
    pub struct LyricsView(ObjectSubclass<imp::LyricsView>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl LyricsView {
    pub fn new(ctx: &Rc<AppContext>) -> Self {
        let view: Self = glib::Object::new();
        view.add_css_class("mp-lyrics");
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .valign(gtk::Align::Start)
            .build();
        list.update_property(&[gtk::accessible::Property::Label("Lyrics")]);
        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&list)
            .vexpand(true)
            .build();
        let missing = adw::StatusPage::builder()
            .icon_name(icons::LYRICS)
            .title("No Lyrics")
            .description(NO_LYRICS)
            .css_classes(["compact"])
            .build();
        let searching = adw::StatusPage::builder()
            .title("Searching for Lyrics")
            .css_classes(["compact"])
            .build();
        searching.set_paintable(Some(&adw::SpinnerPaintable::new(Some(&searching))));
        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .transition_duration(200)
            .build();
        stack.add_named(&gtk::Box::default(), Some("blank"));
        stack.add_named(&scrolled, Some("lines"));
        stack.add_named(&missing, Some("missing"));
        stack.add_named(&searching, Some("searching"));
        view.set_child(Some(&stack));

        let imp = view.imp();
        let scroll = adw::TimedAnimation::builder()
            .widget(&view)
            .value_from(0.0)
            .value_to(0.0)
            .duration(SCROLL_MS)
            .easing(adw::Easing::EaseOutCubic)
            .target(&adw::PropertyAnimationTarget::new(
                &scrolled.vadjustment(),
                "value",
            ))
            .build();
        assert!(imp.scroll.set(scroll).is_ok(), "set once in new");
        assert!(imp.ctx.set(ctx.clone()).is_ok(), "set once in new");
        let parts = Parts {
            stack,
            scrolled,
            list,
            missing,
        };
        assert!(imp.parts.set(parts).is_ok(), "set once in new");

        view.connect_scrolling();
        view.follow_player();
        view.on_position();
        view
    }

    /// Shows `lyrics`, or a note that the song has none, saying whether plugins
    /// `searched_online` too.
    pub fn set_lyrics(&self, lyrics: Option<Rc<Lyrics>>, searched_online: bool) {
        let parts = self.parts();
        parts.missing.set_description(Some(if searched_online {
            NONE_FOUND_ONLINE
        } else {
            NO_LYRICS
        }));
        parts.list.remove_all();
        self.imp().current.set(None);
        if let Some(lyrics) = &lyrics {
            for line in lyrics.lines() {
                parts.list.append(&line_row(&line.text, lyrics.is_synced()));
            }
            set_css_class(&parts.list, "mp-lyrics-synced", lyrics.is_synced());
        }
        parts.scrolled.vadjustment().set_value(0.0);
        parts
            .stack
            .set_visible_child_name(if lyrics.is_some() { "lines" } else { "missing" });
        self.imp().lyrics.replace(lyrics);
        self.resume_following();
        self.update_line();
    }

    /// Shows nothing, such as while the lyrics load.
    pub fn clear(&self) {
        self.show_only("blank");
    }

    /// Shows that plugins are looking for the lyrics online.
    pub fn show_searching(&self) {
        self.show_only("searching");
    }

    fn show_only(&self, page: &str) {
        self.imp().lyrics.replace(None);
        self.imp().current.set(None);
        let parts = self.parts();
        parts.list.remove_all();
        parts.stack.set_visible_child_name(page);
    }

    fn ctx(&self) -> &Rc<AppContext> {
        self.imp().ctx.get().expect("set in new")
    }

    fn parts(&self) -> &Parts {
        self.imp().parts.get().expect("set in new")
    }

    fn follow_player(&self) {
        let player = &self.ctx().player;
        for handler in [
            player.connect_position_notify(follow(self, Self::on_position)),
            player.connect_state_notify(follow(self, Self::on_position)),
        ] {
            disconnect_on_destroy(self, player, handler);
        }

        let parts = self.parts();
        let view = self.downgrade();
        parts.list.connect_row_activated(move |_, row| {
            if let Some(view) = view.upgrade() {
                view.jump_to(row.index());
            }
        });
        // Rows are measured only once laid out, which can be after they should scroll.
        let view = self.downgrade();
        parts.scrolled.vadjustment().connect_changed(move |_| {
            if let Some(view) = view.upgrade()
                && view.imp().scroll_pending.get()
            {
                view.scroll_to_current(false);
            }
        });
        let view = self.downgrade();
        parts
            .scrolled
            .vadjustment()
            .connect_value_changed(move |_| {
                if let Some(view) = view.upgrade() {
                    view.queue_draw();
                }
            });
    }

    /// Scrolling by hand, with a wheel, touchpad, touch or the scrollbar, stops the
    /// lyrics following the song for a while.
    fn connect_scrolling(&self) {
        let scrolled = &self.parts().scrolled;
        let wheel = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        wheel.set_propagation_phase(gtk::PropagationPhase::Capture);
        let view = self.downgrade();
        wheel.connect_scroll(move |_, _, _| {
            if let Some(view) = view.upgrade() {
                view.stop_following();
            }
            glib::Propagation::Proceed
        });
        scrolled.add_controller(wheel);

        let drag = gtk::GestureDrag::new();
        drag.set_propagation_phase(gtk::PropagationPhase::Capture);
        let view = self.downgrade();
        drag.connect_drag_update(move |_, _, y| {
            if let Some(view) = view.upgrade()
                && y.abs() > 4.0
            {
                view.stop_following();
            }
        });
        scrolled.add_controller(drag);
    }

    fn stop_following(&self) {
        let imp = self.imp();
        if let Some(scroll) = imp.scroll.get() {
            scroll.pause();
        }
        if let Some(source) = imp.follow_again.take() {
            source.remove();
        }
        let view = self.downgrade();
        let source = glib::timeout_add_local_once(FOLLOW_AGAIN_AFTER, move || {
            if let Some(view) = view.upgrade() {
                view.imp().follow_again.take();
                view.scroll_to_current(true);
            }
        });
        imp.follow_again.replace(Some(source));
    }

    fn resume_following(&self) {
        if let Some(source) = self.imp().follow_again.take() {
            source.remove();
        }
    }

    fn is_following(&self) -> bool {
        self.imp().follow_again.borrow().is_none()
    }

    fn jump_to(&self, index: i32) {
        let lyrics = self.imp().lyrics.borrow().clone();
        let start = usize::try_from(index)
            .ok()
            .and_then(|index| lyrics?.lines().get(index)?.start);
        if let Some(start) = start {
            self.resume_following();
            self.ctx().player.execute(PlayerCommand::SeekTo(start));
        }
    }

    fn on_position(&self) {
        let position = self.ctx().player.position();
        self.imp().reported.set((position, Some(Instant::now())));
        self.update_line();
    }

    /// Where the song is now: the last reported position, plus the time since while
    /// it plays.
    fn position(&self) -> Duration {
        let (reported, at) = self.imp().reported.get();
        let mut position = Duration::from_millis(reported);
        if self.ctx().player.state() == PlayerState::Playing
            && let Some(at) = at
        {
            position += at.elapsed().min(POSITION_REPORTS * 4);
        }
        position
    }

    /// Marks the line being sung, and wakes up for the next one if it starts before
    /// the next position report.
    fn update_line(&self) {
        let imp = self.imp();
        if let Some(source) = imp.next_line.take() {
            source.remove();
        }
        let Some(lyrics) = imp.lyrics.borrow().clone() else {
            return;
        };
        let position = self.position();
        let line = lyrics.line_at(position);
        if line != imp.current.get() {
            self.set_current(line);
        }
        let playing = self.ctx().player.state() == PlayerState::Playing;
        if let Some(next) = lyrics.next_start(position)
            && playing
            && next - position < POSITION_REPORTS + POSITION_REPORTS / 4
        {
            let view = self.downgrade();
            let wait = next - position + Duration::from_millis(1);
            let source = glib::timeout_add_local_once(wait, move || {
                if let Some(view) = view.upgrade() {
                    view.imp().next_line.take();
                    view.update_line();
                }
            });
            imp.next_line.replace(Some(source));
        }
    }

    fn set_current(&self, line: Option<usize>) {
        let list = &self.parts().list;
        let row = |index: Option<usize>| {
            index.and_then(|index| list.row_at_index(i32::try_from(index).ok()?))
        };
        if let Some(row) = row(self.imp().current.replace(line)) {
            row.remove_css_class("mp-lyrics-current");
        }
        if let Some(row) = row(line) {
            row.add_css_class("mp-lyrics-current");
        }
        if self.is_following() {
            self.scroll_to_current(true);
        }
    }

    /// Scrolls so the line being sung sits [`CURRENT_LINE_AT`] down the view, or to
    /// the top before the first line.
    fn scroll_to_current(&self, animate: bool) {
        let imp = self.imp();
        let parts = self.parts();
        let adjustment = parts.scrolled.vadjustment();
        let current = imp
            .current
            .get()
            .and_then(|index| parts.list.row_at_index(i32::try_from(index).ok()?));
        let target = match current {
            Some(row) => {
                let Some(bounds) = parts
                    .scrolled
                    .child()
                    .and_then(|viewport| row.compute_bounds(&viewport))
                    .filter(|bounds| bounds.height() > 0.0)
                else {
                    imp.scroll_pending.set(true);
                    return;
                };
                adjustment.value() + f64::from(bounds.y() + bounds.height() / 2.0)
                    - adjustment.page_size() * CURRENT_LINE_AT
            }
            None => 0.0,
        };
        imp.scroll_pending.set(false);
        let target = target.clamp(
            adjustment.lower(),
            (adjustment.upper() - adjustment.page_size()).max(adjustment.lower()),
        );
        let Some(scroll) = imp.scroll.get() else {
            return;
        };
        if animate && self.is_mapped() {
            // Not `reset()`: that puts the scroll back where the last animation began.
            // `play()` restarts it anyway.
            scroll.set_value_from(adjustment.value());
            scroll.set_value_to(target);
            scroll.play();
        } else {
            scroll.pause();
            adjustment.set_value(target);
        }
    }
}

/// A line of lyrics. In timed lyrics, empty lines are pauses, shown as a note, and
/// the others can be clicked; in plain ones, empty lines separate verses.
fn line_row(text: &str, synced: bool) -> gtk::ListBoxRow {
    let pause = text.is_empty();
    let label = gtk::Label::builder()
        .label(if pause && synced { PAUSE_TEXT } else { text })
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .justify(gtk::Justification::Center)
        .css_classes(["mp-lyrics-line"])
        .build();
    if pause {
        label.add_css_class(if synced {
            "mp-lyrics-pause"
        } else {
            "mp-lyrics-break"
        });
    }
    gtk::ListBoxRow::builder()
        .child(&label)
        .activatable(synced && !pause)
        .focusable(synced && !pause)
        .build()
}
