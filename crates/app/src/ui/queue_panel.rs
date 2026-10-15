//! The play queue side panel, shown at the window's end edge: the current track,
//! then the tracks that play after it (see [`UpNextList`]). Songs dragged in from
//! any track list are added to the queue.

use std::cell::Cell;
use std::rc::{Rc, Weak};

use adw::prelude::*;
use gtk::gdk;
use mp_core::{RepeatMode, TrackId};

use crate::context::{AppContext, plain_toast};
use crate::model::TrackObject;
use crate::player::Player;
use crate::ui::icons;
use crate::ui::widgets::collections::{NowPlayingCard, UpNextList};
use crate::ui::widgets::track_list::TrackDrag;
use crate::ui::{disconnect_on_destroy, keep_alive};
use crate::util::{song_count, total_length};

/// The panel placed in the window's queue split view.
pub fn build(ctx: &Rc<AppContext>) -> gtk::Widget {
    let panel = QueuePanel::new(ctx);
    ctx.display
        .bind_property("quality-details", &panel.card, "quality-details")
        .sync_create()
        .build();
    let header = adw::HeaderBar::builder()
        .title_widget(&adw::WindowTitle::new("Queue", ""))
        .css_classes(["flat"])
        .build();
    let view = adw::ToolbarView::new();
    view.add_css_class("mp-queue-panel");
    view.add_top_bar(&header);
    view.set_content(Some(&panel.content));

    panel.connect_signals(&view);
    panel.update_modes();
    panel.refresh();
    keep_alive(&view, panel);
    view.upcast()
}

/// The panel's state, owned by its root widget (see [`keep_alive`]).
struct QueuePanel {
    ctx: Rc<AppContext>,
    /// "queue", or "empty" while nothing is queued.
    content: gtk::Stack,
    card: NowPlayingCard,
    summary: gtk::Label,
    shuffle_indicator: gtk::Image,
    repeat_indicator: gtk::Image,
    clear: gtk::Button,
    /// "list", or "nothing" when the current track is the last one.
    up_next: gtk::Stack,
    list: UpNextList,
    /// The queue changed while the panel was hidden; it refreshes once shown.
    stale: Cell<bool>,
}

impl QueuePanel {
    fn new(ctx: &Rc<AppContext>) -> Rc<Self> {
        let panel = Rc::new(Self {
            ctx: ctx.clone(),
            content: gtk::Stack::builder()
                .transition_type(gtk::StackTransitionType::Crossfade)
                .css_classes(["mp-queue-content"])
                .build(),
            card: NowPlayingCard::new(&ctx.covers),
            summary: gtk::Label::builder()
                .xalign(0.0)
                .css_classes(["mp-queue-summary", "caption", "dim-label", "numeric"])
                .build(),
            shuffle_indicator: mode_indicator(icons::SHUFFLE),
            repeat_indicator: mode_indicator(icons::REPEAT),
            clear: gtk::Button::builder()
                .label("Clear")
                .valign(gtk::Align::Center)
                .tooltip_text("Remove every song after the current one")
                .css_classes(["mp-queue-clear"])
                .build(),
            up_next: gtk::Stack::builder()
                .transition_type(gtk::StackTransitionType::Crossfade)
                .vexpand(true)
                .build(),
            list: UpNextList::new(ctx),
            stale: Cell::default(),
        });
        panel.build_layout();
        panel
    }

    fn build_layout(self: &Rc<Self>) {
        let now = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .css_classes(["mp-queue-now"])
            .build();
        now.append(&section_heading("Now Playing"));
        now.append(&self.card);

        let titles = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .hexpand(true)
            .valign(gtk::Align::Center)
            .build();
        titles.append(&section_heading("Up Next"));
        titles.append(&self.summary);
        let header = gtk::Box::builder()
            .spacing(6)
            .css_classes(["mp-queue-up-next-header"])
            .build();
        header.append(&titles);
        header.append(&self.shuffle_indicator);
        header.append(&self.repeat_indicator);
        header.append(&self.clear);

        self.up_next.add_named(&self.list, Some("list"));
        self.up_next.add_named(&nothing_up_next(), Some("nothing"));

        let queue = gtk::Box::new(gtk::Orientation::Vertical, 0);
        queue.append(&now);
        queue.append(&header);
        queue.append(&self.up_next);
        let empty = adw::StatusPage::builder()
            .icon_name(icons::IN_ORDER)
            .title("Queue Is Empty")
            .description("Play some music, or drag songs here to queue them.")
            .css_classes(["compact"])
            .build();
        self.content.add_named(&queue, Some("queue"));
        self.content.add_named(&empty, Some("empty"));
        self.content.add_controller(self.enqueue_drop_target());
    }

    /// Follows the player until `view`, the panel's root, is destroyed.
    fn connect_signals(self: &Rc<Self>, view: &adw::ToolbarView) {
        let player = &self.ctx.player;
        let weak = Rc::downgrade(self);
        let queue_changed = player.connect_queue_changed(move |_| {
            if let Some(panel) = weak.upgrade() {
                panel.refresh_when_shown();
            }
        });
        // A rescan replaces the objects of changed tracks and drops removed ones.
        let weak = Rc::downgrade(self);
        let library_changed = self.ctx.store.connect_tracks_changed(move |_| {
            if let Some(panel) = weak.upgrade() {
                panel.refresh_when_shown();
            }
        });
        disconnect_on_destroy(view, &self.ctx.store, library_changed);
        let weak = Rc::downgrade(self);
        let track_changed = player.connect_current_track_notify(move |player| {
            if let Some(panel) = weak.upgrade() {
                panel.card.set_track(player.current_track().as_ref());
            }
        });
        let update_modes = |panel: Weak<Self>| {
            move |_: &Player| {
                if let Some(panel) = panel.upgrade() {
                    panel.update_modes();
                }
            }
        };
        let shuffle_changed = player.connect_shuffle_notify(update_modes(Rc::downgrade(self)));
        let repeat_changed = player.connect_repeat_notify(update_modes(Rc::downgrade(self)));
        for handler in [
            queue_changed,
            track_changed,
            shuffle_changed,
            repeat_changed,
        ] {
            disconnect_on_destroy(view, player, handler);
        }

        let weak = Rc::downgrade(self);
        view.connect_map(move |_| {
            if let Some(panel) = weak.upgrade()
                && panel.stale.take()
            {
                panel.refresh();
            }
        });
        let weak = Rc::downgrade(self);
        self.clear.connect_clicked(move |_| {
            if let Some(panel) = weak.upgrade() {
                panel.clear_upcoming();
            }
        });
    }

    /// Refreshes now if the panel is showing, otherwise once it is shown.
    fn refresh_when_shown(&self) {
        if self.content.is_mapped() {
            self.refresh();
        } else {
            self.stale.set(true);
        }
    }

    /// Shows the queue as it is now.
    fn refresh(&self) {
        let player = &self.ctx.player;
        let queue = player.queue_tracks();
        let rows = upcoming_rows(&queue, player.queue_position(), |id| {
            self.ctx.store.track(id)
        });
        let (positions, tracks): (Vec<usize>, Vec<TrackObject>) = rows.into_iter().unzip();
        let has_upcoming = !tracks.is_empty();
        self.summary.set_label(&summary(&tracks));
        self.summary.set_visible(has_upcoming);
        self.list.set_rows(positions, tracks);

        self.card.set_track(player.current_track().as_ref());
        self.clear.set_sensitive(has_upcoming);
        let up_next = if has_upcoming { "list" } else { "nothing" };
        self.up_next.set_visible_child_name(up_next);
        let content = if queue.is_empty() { "empty" } else { "queue" };
        self.content.set_visible_child_name(content);
    }

    /// Shows which of shuffle and repeat are on, next to the "Up Next" heading.
    fn update_modes(&self) {
        let player = &self.ctx.player;
        self.shuffle_indicator.set_visible(player.shuffle());
        set_indicator_text(&self.shuffle_indicator, "Shuffle is on");
        let repeat = match player.repeat_mode() {
            RepeatMode::Off => None,
            RepeatMode::All => Some((icons::REPEAT, "Repeating the queue")),
            RepeatMode::One => Some((icons::REPEAT_ONE, "Repeating the current song")),
        };
        self.repeat_indicator.set_visible(repeat.is_some());
        if let Some((icon, text)) = repeat {
            self.repeat_indicator.set_icon_name(Some(icon));
            set_indicator_text(&self.repeat_indicator, text);
        }
    }

    /// Clears the up-next list, with a toast to bring it back.
    fn clear_upcoming(&self) {
        let player = &self.ctx.player;
        let queue = player.queue_tracks();
        let start = player.queue_position().map_or(queue.len(), |p| p + 1);
        let cleared: Vec<TrackId> = queue[start.min(queue.len())..].to_vec();
        if cleared.is_empty() {
            return;
        }
        player.clear_upcoming();

        let toast = plain_toast(&format!(
            "Cleared {} from the queue",
            song_count(cleared.len())
        ));
        toast.set_button_label(Some("_Undo"));
        let player = player.downgrade();
        toast.connect_button_clicked(move |_| {
            if let Some(player) = player.upgrade() {
                player.enqueue(&cleared);
            }
        });
        self.ctx.show_toast(toast);
    }

    /// Queues songs dropped on the panel from any track list. Rows of the up-next
    /// list itself are left to the list, which moves them.
    fn enqueue_drop_target(self: &Rc<Self>) -> gtk::DropTarget {
        let target = gtk::DropTarget::new(TrackDrag::static_type(), gdk::DragAction::COPY);
        let weak = Rc::downgrade(self);
        target.connect_accept(move |_, drop| {
            drop.formats().contains_type(TrackDrag::static_type())
                && weak
                    .upgrade()
                    .is_some_and(|panel| !panel.list.is_dragging())
        });
        let weak = Rc::downgrade(self);
        target.connect_drop(move |_, value, _, _| {
            let (Some(panel), Ok(TrackDrag(tracks))) = (weak.upgrade(), value.get::<TrackDrag>())
            else {
                return false;
            };
            panel.ctx.player.enqueue(&tracks);
            panel
                .ctx
                .toast(&format!("Added {} to the queue", song_count(tracks.len())));
            true
        });
        target
    }
}

/// The tracks after `position` in `queue`, each with its queue position. Tracks
/// `lookup` can't find (no longer in the library) are left out.
fn upcoming_rows<T>(
    queue: &[TrackId],
    position: Option<usize>,
    lookup: impl Fn(TrackId) -> Option<T>,
) -> Vec<(usize, T)> {
    let start = position.map_or(queue.len(), |position| position + 1);
    queue
        .iter()
        .enumerate()
        .skip(start)
        .filter_map(|(index, &id)| lookup(id).map(|track| (index, track)))
        .collect()
}

/// "12 songs · 48 min".
fn summary(tracks: &[TrackObject]) -> String {
    if tracks.is_empty() {
        return String::new();
    }
    let duration_ms: u64 = tracks.iter().map(TrackObject::duration_ms).sum();
    format!(
        "{} · {}",
        song_count(tracks.len()),
        total_length(duration_ms)
    )
}

fn section_heading(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .css_classes(["mp-queue-heading", "heading"])
        .build()
}

/// A small icon that shows a playback mode is on.
fn mode_indicator(icon_name: &str) -> gtk::Image {
    gtk::Image::builder()
        .icon_name(icon_name)
        .valign(gtk::Align::Center)
        .visible(false)
        .css_classes(["mp-queue-mode"])
        .build()
}

fn set_indicator_text(indicator: &gtk::Image, text: &str) {
    indicator.set_tooltip_text(Some(text));
    indicator.update_property(&[gtk::accessible::Property::Label(text)]);
}

/// What the up-next area shows when the current track is the last one.
fn nothing_up_next() -> gtk::Widget {
    let title = gtk::Label::builder()
        .label("Nothing up next")
        .css_classes(["heading", "dim-label"])
        .build();
    let hint = gtk::Label::builder()
        .label("Use “Add to Queue” in a song’s menu, or drag songs here.")
        .wrap(true)
        .justify(gtk::Justification::Center)
        .css_classes(["caption", "dim-label"])
        .build();
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .valign(gtk::Align::Start)
        .css_classes(["mp-queue-nothing"])
        .build();
    content.append(&title);
    content.append(&hint);
    content.upcast()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(ids: &[i64]) -> Vec<TrackId> {
        ids.iter().copied().map(TrackId).collect()
    }

    #[test]
    fn upcoming_rows_follow_play_order() {
        let queue = ids(&[10, 11, 12, 13]);
        let known = |id: TrackId| (id != TrackId(12)).then_some(id.0);
        assert_eq!(
            upcoming_rows(&queue, Some(0), known),
            vec![(1, 11), (3, 13)]
        );
        assert!(upcoming_rows(&queue, Some(3), known).is_empty());
        assert!(upcoming_rows(&[], None, known).is_empty());
    }

    #[test]
    fn summary_counts_songs_and_time() {
        let tracks: Vec<TrackObject> = (1..=3)
            .map(|id| TrackObject::new(crate::model::test_track(id, "t", "a", "b")))
            .collect();
        assert_eq!(summary(&tracks), "3 songs · 10 min");
        assert_eq!(summary(&[]), "");
    }
}
