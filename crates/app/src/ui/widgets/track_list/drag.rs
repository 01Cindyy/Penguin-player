//! Dragging tracks out of a [`TrackList`], and dropping them back to reorder it.
//!
//! Every list is a drag source: dragging a row (or the selection it belongs to)
//! carries a [`TrackDrag`], which other widgets accept with
//! `gtk::DropTarget::new(TrackDrag::static_type(), gdk::DragAction::COPY)`. Lists built
//! with [`super::TrackListBuilder::on_reorder`] also take their own rows back.
//!
//! The helpers at the end ([`track_drag_source`], [`DropMarker`],
//! [`scroll_during_drag`]) are shared with other lists of tracks, such as the queue.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gdk, glib};
use mp_core::TrackId;

use super::TrackList;
use super::cells::TrackCell;
use crate::model::TrackObject;
use crate::ui::set_css_class;
use crate::util::song_count;

/// Runs with the dragged rows' positions and the position they are dropped before
/// (the row count to drop them at the end).
pub(super) type ReorderHandler = Box<dyn Fn(&TrackList, &[u32], u32)>;

/// The tracks being dragged, in display order. Only understood inside the app.
#[derive(Debug, Clone, Default, PartialEq, Eq, glib::Boxed)]
#[boxed_type(name = "MpTrackDrag")]
pub struct TrackDrag(pub Vec<TrackId>);

/// A list's drag-and-drop state.
#[derive(Default)]
pub(super) struct DragState {
    pub(super) on_reorder: RefCell<Option<ReorderHandler>>,
    /// Positions of the rows being dragged out of this list.
    dragged: RefCell<Vec<u32>>,
    marker: DropMarker,
}

impl TrackList {
    fn is_reorderable(&self) -> bool {
        self.imp().drag.on_reorder.borrow().is_some()
    }

    /// Makes `widget`, one cell of the row that `cell` shows, a drag source, and a
    /// drop target in reorderable lists.
    pub(super) fn attach_drag_and_drop(&self, widget: &TrackCell, cell: &gtk::ColumnViewCell) {
        let list = self.downgrade();
        let row = cell.downgrade();
        let prepare = move || {
            let (list, row) = (list.upgrade()?, row.upgrade()?);
            list.start_drag(row.position())
        };
        let list = self.downgrade();
        let end = move || {
            if let Some(list) = list.upgrade() {
                list.end_drag();
            }
        };
        widget.add_controller(track_drag_source(prepare, end));

        if self.is_reorderable() {
            // The cell's padding moves into the widget (see collections.css), so the
            // drop target covers the whole row.
            widget.add_css_class("mp-reorder-cell");
            widget.add_controller(self.reorder_target(widget, cell));
        }
    }

    /// Accepts this list's own rows: the drop lands before or after the row under the
    /// pointer, depending on which half of it the pointer is in.
    fn reorder_target(&self, widget: &TrackCell, cell: &gtk::ColumnViewCell) -> gtk::DropTarget {
        let target = gtk::DropTarget::new(TrackDrag::static_type(), gdk::DragAction::MOVE);
        let list = self.downgrade();
        target.connect_accept(move |_, _| list.upgrade().is_some_and(|list| list.has_own_drag()));

        let list = self.downgrade();
        let row = cell.downgrade();
        let cell_widget = widget.downgrade();
        let drop_point = Rc::new(move |y: f64| {
            let (list, row, widget) = (list.upgrade()?, row.upgrade()?, cell_widget.upgrade()?);
            let after = drops_after(&widget, y);
            Some((list, widget, row.position() + u32::from(after), after))
        });

        let point = drop_point.clone();
        target.connect_motion(move |_, _, y| match point(y) {
            Some((list, widget, _, after)) => {
                list.imp().drag.marker.mark(&widget, after);
                gdk::DragAction::MOVE
            }
            None => gdk::DragAction::empty(),
        });
        let list = self.downgrade();
        target.connect_leave(move |_| {
            if let Some(list) = list.upgrade() {
                list.imp().drag.marker.clear();
            }
        });
        target.connect_drop(move |_, _, _, y| {
            let Some((list, _, insert_at, _)) = drop_point(y) else {
                return false;
            };
            list.imp().drag.marker.clear();
            let dragged = list.imp().drag.dragged.take();
            if let Some(handler) = list.imp().drag.on_reorder.borrow().as_ref() {
                handler(&list, &dragged, insert_at);
            }
            true
        });
        target
    }

    /// Drags the selection, after selecting the row at `position` alone if it
    /// wasn't part of it.
    fn start_drag(&self, position: u32) -> Option<(TrackDrag, String)> {
        let imp = self.imp();
        if !imp.selection.is_selected(position) {
            imp.selection.select_item(position, true);
        }
        let positions = selected_positions(&imp.selection.selection());
        let tracks: Vec<TrackObject> = positions
            .iter()
            .filter_map(|&position| imp.sort_model.item(position).and_downcast())
            .collect();
        let label = match tracks.as_slice() {
            [] => return None,
            [track] => track.title(),
            _ => song_count(tracks.len()),
        };
        imp.drag.dragged.replace(positions);
        let ids = tracks.iter().map(TrackObject::track_id).collect();
        Some((TrackDrag(ids), label))
    }

    fn end_drag(&self) {
        self.imp().drag.dragged.take();
        self.imp().drag.marker.clear();
    }

    fn has_own_drag(&self) -> bool {
        !self.imp().drag.dragged.borrow().is_empty()
    }

    /// Reorderable lists scroll while a row is dragged near their edges.
    pub(super) fn enable_drag_scrolling(&self) {
        let scrolled = self
            .imp()
            .column_view
            .parent()
            .and_downcast::<gtk::ScrolledWindow>();
        if let Some(scrolled) = scrolled {
            scroll_during_drag(&scrolled);
        }
    }
}

/// A drag source for tracks. When a drag starts, `prepare` returns what to drag and
/// a label for the pill that follows the pointer (`mp-drag-icon`), or `None` not to
/// drag; `end` runs once the drag is over. Touch drags are left to scrolling.
pub fn track_drag_source(
    prepare: impl Fn() -> Option<(TrackDrag, String)> + 'static,
    end: impl Fn() + 'static,
) -> gtk::DragSource {
    let source = gtk::DragSource::new();
    source.set_actions(gdk::DragAction::COPY | gdk::DragAction::MOVE);
    let label = Rc::new(RefCell::new(String::new()));

    let next_label = label.clone();
    source.connect_prepare(move |source, _, _| {
        if started_by_touch(source) {
            return None;
        }
        let (tracks, text) = prepare()?;
        next_label.replace(text);
        Some(gdk::ContentProvider::for_value(&tracks.to_value()))
    });
    source.connect_drag_begin(move |_, drag| {
        let text = label.borrow();
        let icon = gtk::Label::builder()
            .label(text.as_str())
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(36)
            .css_classes(["mp-drag-icon"])
            .build();
        gtk::DragIcon::for_drag(drag).set_child(Some(&icon));
    });
    source.connect_drag_end(move |_, _, _| end());
    source
}

/// Shows where a drop will land: a line above or below one list row at a time
/// (the row gets `mp-drop-before` or `mp-drop-after`).
#[derive(Debug, Default)]
pub struct DropMarker {
    row: RefCell<Option<gtk::Widget>>,
}

impl DropMarker {
    /// Marks the list row (a `row` CSS node) that contains `widget`.
    pub fn mark(&self, widget: &impl IsA<gtk::Widget>, after: bool) {
        let Some(row) = row_of(widget.upcast_ref()) else {
            return;
        };
        if self.row.borrow().as_ref() != Some(&row) {
            self.clear();
            self.row.replace(Some(row.clone()));
        }
        set_css_class(&row, "mp-drop-before", !after);
        set_css_class(&row, "mp-drop-after", after);
    }

    pub fn clear(&self) {
        if let Some(row) = self.row.take() {
            row.remove_css_class("mp-drop-before");
            row.remove_css_class("mp-drop-after");
        }
    }
}

/// Whether a drop at `y` in `widget` lands after it rather than before it.
pub fn drops_after(widget: &impl IsA<gtk::Widget>, y: f64) -> bool {
    y > f64::from(widget.height()) / 2.0
}

/// Scrolls `scrolled` while something is dragged close to its top or bottom edge, so
/// a drop can reach any part of a long list.
pub fn scroll_during_drag(scrolled: &gtk::ScrolledWindow) {
    /// How close to an edge (in pixels) scrolling starts, and its top speed there.
    const EDGE: f64 = 56.0;
    const MAX_SPEED: f64 = 18.0;

    let speed = Rc::new(Cell::new(0.0));
    let ticking: Rc<RefCell<Option<gtk::TickCallbackId>>> = Rc::default();
    let motion = gtk::DropControllerMotion::new();

    let (step, tick) = (speed.clone(), ticking.clone());
    motion.connect_motion(move |motion, _, y| {
        let Some(scrolled) = motion.widget() else {
            return;
        };
        let height = f64::from(scrolled.height());
        let closeness = if y < EDGE {
            -(EDGE - y) / EDGE
        } else if y > height - EDGE {
            (y - (height - EDGE)) / EDGE
        } else {
            0.0
        };
        step.set(closeness.clamp(-1.0, 1.0) * MAX_SPEED);
        if step.get() != 0.0 && tick.borrow().is_none() {
            let step = step.clone();
            let id = scrolled.add_tick_callback(move |scrolled, _| {
                if let Some(scrolled) = scrolled.downcast_ref::<gtk::ScrolledWindow>() {
                    let adjustment = scrolled.vadjustment();
                    adjustment.set_value(adjustment.value() + step.get());
                }
                glib::ControlFlow::Continue
            });
            tick.replace(Some(id));
        } else if step.get() == 0.0
            && let Some(id) = tick.take()
        {
            id.remove();
        }
    });
    motion.connect_leave(move |_| {
        speed.set(0.0);
        if let Some(id) = ticking.take() {
            id.remove();
        }
    });
    scrolled.add_controller(motion);
}

fn started_by_touch(source: &gtk::DragSource) -> bool {
    source
        .current_event_device()
        .is_some_and(|device| device.source() == gdk::InputSource::Touchscreen)
}

/// The list row (a `row` CSS node) that contains `widget`.
fn row_of(widget: &gtk::Widget) -> Option<gtk::Widget> {
    std::iter::successors(Some(widget.clone()), gtk::Widget::parent)
        .find(|widget| widget.css_name() == "row")
}

pub(super) fn selected_positions(selection: &gtk::Bitset) -> Vec<u32> {
    let Some((rest, first)) = gtk::BitsetIter::init_first(selection) else {
        return Vec::new();
    };
    std::iter::once(first).chain(rest).collect()
}
