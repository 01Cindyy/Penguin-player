//! The playing song's lyrics. Timed lyrics follow the song: the line being sung
//! stands out (`mp-lyrics-current`) and is kept in the middle of the view, lines already
//! sung are dimmer (`mp-lyrics-sung`), and clicking a line jumps there. Scrolling the
//! lyrics by hand stops them following for a few seconds.
//!
//! GTK shows a scrollbar whenever anything scrolls, so `mp-lyrics-scrollbar-hidden` hides
//! it again after automatic scrolling, until the pointer moves or the user scrolls.

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
/// Lines change on time even between the player's position reports, which come
/// about this often.
const POSITION_REPORTS: Duration = Duration::from_millis(250);
/// Height of the faded top and bottom edges, as a fraction of the view's height.
const FADE: f32 = 0.2;
const HIDDEN_SCROLLBAR: &str = "mp-lyrics-scrollbar-hidden";
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
        /// Whether the next frame will fit the lines to the view's size.
        pub(super) fitting: Cell<bool>,
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
            let top = above.clamp(0.0, FADE * height);
            let bottom = below.clamp(0.0, FADE * height);
            if parts.stack.visible_child_name().as_deref() != Some("lines")
                || height <= 0.0
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
            .css_classes([HIDDEN_SCROLLBAR])
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
        parts.scrolled.add_css_class(HIDDEN_SCROLLBAR);
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
        self.update_insets();
        self.resume_following();
        self.update_line();
        self.scroll_to_current(false);
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
        // The view changed size, or the lines did.
        let view = self.downgrade();
        parts.scrolled.vadjustment().connect_changed(move |_| {
            if let Some(view) = view.upgrade() {
                view.fit_next_frame();
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

        // GTK also reports the pointer as moving whenever the layout changes under it,
        // so this checks that it really has.
        let pointer = gtk::EventControllerMotion::new();
        pointer.set_propagation_phase(gtk::PropagationPhase::Capture);
        let last_seen = Cell::new(None);
        let weak = scrolled.downgrade();
        pointer.connect_motion(move |_, x, y| {
            if last_seen.replace(Some((x, y))) != Some((x, y))
                && let Some(scrolled) = weak.upgrade()
            {
                scrolled.remove_css_class(HIDDEN_SCROLLBAR);
            }
        });
        scrolled.add_controller(pointer);
    }

    fn stop_following(&self) {
        let imp = self.imp();
        self.parts().scrolled.remove_css_class(HIDDEN_SCROLLBAR);
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
        let previous = self.imp().current.replace(line);
        if let Some(row) = row(previous) {
            row.remove_css_class("mp-lyrics-current");
        }
        if let Some(row) = row(line) {
            row.add_css_class("mp-lyrics-current");
        }
        // Only the lines between the old and new place change, which may be many after
        // a seek.
        let (old_end, new_end) = (previous.unwrap_or(0), line.unwrap_or(0));
        for index in old_end.min(new_end)..old_end.max(new_end) {
            if let Some(row) = row(Some(index)) {
                set_css_class(&row, "mp-lyrics-sung", index < new_end);
            }
        }
        if self.is_following() {
            self.scroll_to_current(true);
        }
    }

    fn is_synced(&self) -> bool {
        let lyrics = self.imp().lyrics.borrow();
        lyrics.as_ref().is_some_and(|lyrics| lyrics.is_synced())
    }

    /// Fits the lines to the view's size and centres the line being sung again, at the
    /// start of the next frame: sizes mustn't change in the middle of size allocation.
    fn fit_next_frame(&self) {
        if self.imp().fitting.replace(true) {
            return;
        }
        self.add_tick_callback(|view, _| {
            view.imp().fitting.set(false);
            // If this changes the insets, the lines move once they are laid out, which
            // brings us back here.
            if !view.update_insets() && view.is_following() {
                view.scroll_to_current(false);
            }
            glib::ControlFlow::Break
        });
    }

    /// Leaves half a view of room above and below timed lyrics, so their first and last
    /// lines can reach the middle too. Returns whether that changed anything.
    fn update_insets(&self) -> bool {
        let list = &self.parts().list;
        let inset = if self.is_synced() {
            self.height() / 2
        } else {
            0
        };
        let changed = list.margin_top() != inset;
        list.set_margin_top(inset);
        list.set_margin_bottom(inset);
        changed
    }

    /// Scrolls so the line being sung, or the first until then, sits in the middle of
    /// the view. Plain lyrics stay where the reader left them.
    fn scroll_to_current(&self, animate: bool) {
        if !self.is_synced() {
            return;
        }
        let imp = self.imp();
        let parts = self.parts();
        parts.scrolled.add_css_class(HIDDEN_SCROLLBAR);
        let adjustment = parts.scrolled.vadjustment();
        let current = i32::try_from(imp.current.get().unwrap_or(0))
            .ok()
            .and_then(|index| parts.list.row_at_index(index));
        let target = match current {
            Some(row) => {
                let Some(bounds) = parts
                    .scrolled
                    .child()
                    .and_then(|viewport| row.compute_bounds(&viewport))
                    .filter(|bounds| bounds.height() > 0.0)
                else {
                    // Rows are measured only once laid out.
                    self.fit_next_frame();
                    return;
                };
                adjustment.value() + f64::from(bounds.y() + bounds.height() / 2.0)
                    - adjustment.page_size() / 2.0
            }
            None => 0.0,
        };
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
