//! The queue panel's "Up Next" list: the tracks after the current one, in play
//! order. Double-click or Enter plays a row, its button or Delete removes it, and
//! rows can be dragged (or moved with Alt+↑ and Alt+↓) to a new place in the queue,
//! or dragged onto a playlist.

use std::cell::{OnceCell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gdk, gio, glib};

use super::{DistinctRows, QueueRow, reorder};
use crate::context::AppContext;
use crate::model::{TrackObject, sync_list_store};
use crate::ui::widgets::track_list::{
    DropMarker, TrackDrag, drops_after, scroll_during_drag, track_drag_source,
};

type ListCommand = fn(&UpNextList);

mod imp {
    use super::*;

    pub struct UpNextList {
        pub(super) ctx: OnceCell<Rc<AppContext>>,
        pub(super) tracks: gio::ListStore,
        pub(super) selection: gtk::SingleSelection,
        pub(super) list_view: gtk::ListView,
        /// The queue position (in play order) of each row.
        pub(super) positions: RefCell<Vec<usize>>,
        pub(super) distinct: RefCell<DistinctRows>,
        /// The row being dragged, while one is. Kept by its object rather than its
        /// index, as rows move up when the next track starts during the drag.
        pub(super) dragged: RefCell<Option<TrackObject>>,
        pub(super) marker: DropMarker,
    }

    impl Default for UpNextList {
        fn default() -> Self {
            let tracks = gio::ListStore::new::<TrackObject>();
            let selection = gtk::SingleSelection::builder()
                .model(&tracks)
                .autoselect(false)
                .can_unselect(true)
                .build();
            Self {
                ctx: OnceCell::new(),
                list_view: gtk::ListView::new(
                    Some(selection.clone()),
                    None::<gtk::ListItemFactory>,
                ),
                tracks,
                selection,
                positions: RefCell::default(),
                distinct: RefCell::default(),
                dragged: RefCell::default(),
                marker: DropMarker::default(),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for UpNextList {
        const NAME: &'static str = "MpUpNextList";
        type Type = super::UpNextList;
        type ParentType = adw::Bin;
    }

    impl ObjectImpl for UpNextList {
        fn constructed(&self) {
            self.parent_constructed();
            self.list_view.set_tab_behavior(gtk::ListTabBehavior::Item);
            self.list_view.add_css_class("mp-queue-list");
            self.list_view
                .update_property(&[gtk::accessible::Property::Label("Up Next")]);
            let scrolled = gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .vexpand(true)
                .child(&self.list_view)
                .build();
            scroll_during_drag(&scrolled);
            self.obj().set_child(Some(&scrolled));
        }
    }

    impl WidgetImpl for UpNextList {}
    impl BinImpl for UpNextList {}
}

glib::wrapper! {
    /// See the [module documentation](self). Rows are [`QueueRow`]s in a recycled
    /// `listview.mp-queue-list`.
    pub struct UpNextList(ObjectSubclass<imp::UpNextList>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl UpNextList {
    pub fn new(ctx: &Rc<AppContext>) -> Self {
        let list: Self = glib::Object::new();
        assert!(
            list.imp().ctx.set(ctx.clone()).is_ok(),
            "a new list has no context yet"
        );
        let list_view = &list.imp().list_view;
        list_view.set_factory(Some(&list.row_factory()));
        let weak = list.downgrade();
        list_view.connect_activate(move |_, row| {
            if let Some(list) = weak.upgrade() {
                list.play_row(row);
            }
        });
        list_view.add_controller(list.keyboard_shortcuts());
        list
    }

    /// Shows `tracks`, each at queue position `positions[i]`. Rows that stay the same
    /// are left alone, so a track starting only removes the top row, even in a queue
    /// of thousands.
    pub fn set_rows(&self, positions: Vec<usize>, tracks: Vec<TrackObject>) {
        let imp = self.imp();
        imp.positions.replace(positions);
        let rows = imp.distinct.borrow_mut().apply(tracks);
        sync_list_store(&imp.tracks, &rows);
    }

    /// Whether one of this list's rows is being dragged.
    pub fn is_dragging(&self) -> bool {
        self.imp().dragged.borrow().is_some()
    }

    fn ctx(&self) -> &Rc<AppContext> {
        self.imp()
            .ctx
            .get()
            .expect("lists are created through UpNextList::new")
    }

    /// The queue position shown by `row`, if the row exists.
    fn position_of(&self, row: u32) -> Option<usize> {
        self.imp().positions.borrow().get(row as usize).copied()
    }

    fn play_row(&self, row: u32) {
        if let Some(position) = self.position_of(row) {
            self.ctx().player.jump_to(position);
        }
    }

    fn remove_row(&self, row: u32) {
        if let Some(position) = self.position_of(row) {
            self.ctx().player.remove_from_queue(position);
        }
    }

    /// Removes the selected row, then selects the one that took its place so pressing
    /// Delete again keeps going.
    fn remove_selected(&self) {
        let imp = self.imp();
        let row = imp.selection.selected();
        if row == gtk::INVALID_LIST_POSITION {
            return;
        }
        self.remove_row(row);
        let remaining = imp.tracks.n_items();
        if remaining > 0 {
            let flags = gtk::ListScrollFlags::FOCUS | gtk::ListScrollFlags::SELECT;
            imp.list_view.scroll_to(row.min(remaining - 1), flags, None);
        }
    }

    /// Moves row `row` to just before row `before`; past the last row is the end of
    /// the queue.
    fn move_row(&self, row: u32, before: u32) {
        let Some(from) = self.position_of(row) else {
            return;
        };
        let player = &self.ctx().player;
        let before = self
            .position_of(before)
            .unwrap_or_else(|| player.queue_len());
        if let Some(to) = reorder::queue_move(from, before) {
            player.move_in_queue(from, to);
        }
    }

    /// Moves the dragged row to just before row `before`, unless it has left the list
    /// meanwhile (it started playing, say).
    fn move_dragged_row(&self, before: u32) {
        let imp = self.imp();
        imp.marker.clear();
        let dragged = imp.dragged.take();
        if let Some(row) = dragged.and_then(|track| imp.tracks.find(&track)) {
            self.move_row(row, before);
        }
    }

    /// Moves the selected row one place up or down, keeping it selected and focused.
    fn move_selected(&self, up: bool) {
        let imp = self.imp();
        let row = imp.selection.selected();
        let last = imp.tracks.n_items().saturating_sub(1);
        if row == gtk::INVALID_LIST_POSITION || (up && row == 0) || (!up && row >= last) {
            return;
        }
        let (before, moved_to) = if up {
            (row - 1, row - 1)
        } else {
            (row + 2, row + 1)
        };
        self.move_row(row, before);
        let flags = gtk::ListScrollFlags::FOCUS | gtk::ListScrollFlags::SELECT;
        imp.list_view.scroll_to(moved_to, flags, None);
    }

    /// Delete removes the selected row; Alt+↑ and Alt+↓ move it.
    fn keyboard_shortcuts(&self) -> gtk::ShortcutController {
        let controller = gtk::ShortcutController::new();
        let shortcuts: [(&str, ListCommand); 3] = [
            ("Delete|KP_Delete", Self::remove_selected),
            ("<Alt>Up", |list| list.move_selected(true)),
            ("<Alt>Down", |list| list.move_selected(false)),
        ];
        for (trigger, run) in shortcuts {
            let weak = self.downgrade();
            let action = gtk::CallbackAction::new(move |_, _| {
                if let Some(list) = weak.upgrade() {
                    run(&list);
                }
                glib::Propagation::Stop
            });
            controller.add_shortcut(gtk::Shortcut::new(
                gtk::ShortcutTrigger::parse_string(trigger),
                Some(action),
            ));
        }
        controller
    }

    /// Rows are created once per visible row and rebound as the list scrolls.
    fn row_factory(&self) -> gtk::SignalListItemFactory {
        let factory = gtk::SignalListItemFactory::new();
        let weak = self.downgrade();
        factory.connect_setup(move |_, item| {
            let (Some(list), Some(item)) = (weak.upgrade(), item.downcast_ref::<gtk::ListItem>())
            else {
                return;
            };
            let row = QueueRow::new(&list.ctx().covers);
            list.attach_row_controls(&row, item);
            item.set_child(Some(&row));
        });
        factory.connect_bind(|_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let row = item.child().and_downcast::<QueueRow>();
            let track = item.item().and_downcast::<TrackObject>();
            if let (Some(row), Some(track)) = (row, track) {
                row.set_track(&track);
                item.set_accessible_label(&format!("{} by {}", track.title(), track.artist()));
            }
        });
        factory
    }

    fn attach_row_controls(&self, row: &QueueRow, item: &gtk::ListItem) {
        let weak = self.downgrade();
        let list_item = item.downgrade();
        row.remove_button().connect_clicked(move |_| {
            if let (Some(list), Some(item)) = (weak.upgrade(), list_item.upgrade()) {
                list.remove_row(item.position());
            }
        });
        row.add_controller(self.row_drag_source(item));
        row.add_controller(self.row_drop_target(row, item));
    }

    /// Dragging a row moves it within the queue, or adds it to a playlist dropped on.
    fn row_drag_source(&self, item: &gtk::ListItem) -> gtk::DragSource {
        let weak = self.downgrade();
        let list_item = item.downgrade();
        let prepare = move || {
            let (list, item) = (weak.upgrade()?, list_item.upgrade()?);
            let track = item.item().and_downcast::<TrackObject>()?;
            list.imp().dragged.replace(Some(track.clone()));
            Some((TrackDrag(vec![track.track_id()]), track.title()))
        };
        let weak = self.downgrade();
        let end = move || {
            if let Some(list) = weak.upgrade() {
                list.imp().dragged.take();
                list.imp().marker.clear();
            }
        };
        track_drag_source(prepare, end)
    }

    /// Takes this list's own rows: they land before or after the row under the
    /// pointer, depending on which half of it the pointer is in. Songs dragged in
    /// from elsewhere are left to the queue panel.
    fn row_drop_target(&self, row: &QueueRow, item: &gtk::ListItem) -> gtk::DropTarget {
        let target = gtk::DropTarget::new(TrackDrag::static_type(), gdk::DragAction::MOVE);
        let weak = self.downgrade();
        target.connect_accept(move |_, _| weak.upgrade().is_some_and(|list| list.is_dragging()));

        let weak = self.downgrade();
        let list_item = item.downgrade();
        let row = row.downgrade();
        let drop_point = Rc::new(move |y: f64| {
            let (list, item, row) = (weak.upgrade()?, list_item.upgrade()?, row.upgrade()?);
            let after = drops_after(&row, y);
            Some((list, row, item.position() + u32::from(after), after))
        });

        let point = drop_point.clone();
        target.connect_motion(move |_, _, y| match point(y) {
            Some((list, row, _, after)) => {
                list.imp().marker.mark(&row, after);
                gdk::DragAction::MOVE
            }
            None => gdk::DragAction::empty(),
        });
        let weak = self.downgrade();
        target.connect_leave(move |_| {
            if let Some(list) = weak.upgrade() {
                list.imp().marker.clear();
            }
        });
        target.connect_drop(move |_, _, _, y| match drop_point(y) {
            Some((list, _, before, _)) => {
                list.move_dragged_row(before);
                true
            }
            None => false,
        });
        target
    }
}
