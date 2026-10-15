//! A sortable, virtualised list of tracks that every page can reuse.
//!
//! ```ignore
//! let list = TrackList::builder(ctx)
//!     .columns(&[TrackColumn::Number, TrackColumn::Title, TrackColumn::Duration])
//!     .build(&model);
//! ```
//!
//! Rows are recycled as the list scrolls, so it stays smooth with tens of thousands of
//! tracks. Double-click or Enter plays the list from that row; right-click or a long
//! press opens the shared track menu for the selected rows. Rows can be dragged out
//! as a [`TrackDrag`] (see the `drag` module).

mod cells;
mod drag;

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gdk, gio, glib, graphene};
use mp_core::TrackId;

use cells::TrackCell;
pub use cells::TrackColumn;
pub use drag::{DropMarker, TrackDrag, drops_after, scroll_during_drag, track_drag_source};

use crate::context::AppContext;
use crate::model::{TrackObject, track_ids_of};
use crate::ui::icons;
use crate::ui::track_actions::{self, MenuSongs};
use crate::ui::{context_menu, mark_narrow_while, max_width_breakpoint, show_empty_state};

type ActivateHandler = Box<dyn Fn(&TrackList, u32)>;

/// Shown on the empty page unless a spinner replaces it (see [`TrackList::set_empty_state`]).
const EMPTY_ICON: &str = icons::SONG;

mod imp {
    use super::*;

    pub struct TrackList {
        pub(super) ctx: OnceCell<Rc<AppContext>>,
        /// Hides secondary columns when the list is narrow.
        pub(super) breakpoints: adw::BreakpointBin,
        pub(super) stack: gtk::Stack,
        pub(super) column_view: gtk::ColumnView,
        pub(super) sort_model: gtk::SortListModel,
        pub(super) selection: gtk::MultiSelection,
        pub(super) empty_page: adw::StatusPage,
        pub(super) popover: gtk::PopoverMenu,
        pub(super) menu_section: RefCell<Option<gio::MenuModel>>,
        pub(super) on_activate: RefCell<Option<ActivateHandler>>,
        /// The row the context menu was opened for.
        pub(super) menu_position: Cell<Option<u32>>,
        pub(super) drag: drag::DragState,
    }

    impl Default for TrackList {
        fn default() -> Self {
            let sort_model = gtk::SortListModel::new(None::<gio::ListModel>, None::<gtk::Sorter>);
            let selection = gtk::MultiSelection::new(Some(sort_model.clone()));
            Self {
                ctx: OnceCell::new(),
                breakpoints: adw::BreakpointBin::new(),
                stack: gtk::Stack::new(),
                column_view: gtk::ColumnView::new(Some(selection.clone())),
                sort_model,
                selection,
                empty_page: adw::StatusPage::new(),
                popover: gtk::PopoverMenu::from_model(None::<&gio::MenuModel>),
                menu_section: RefCell::default(),
                on_activate: RefCell::default(),
                menu_position: Cell::default(),
                drag: drag::DragState::default(),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TrackList {
        const NAME: &'static str = "MpTrackList";
        type Type = super::TrackList;
        type ParentType = adw::Bin;
    }

    impl ObjectImpl for TrackList {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.add_css_class("mp-track-list");

            self.column_view.set_show_row_separators(false);
            self.column_view.set_show_column_separators(false);
            self.column_view
                .set_tab_behavior(gtk::ListTabBehavior::Item);
            self.column_view.set_reorderable(false);
            self.column_view.add_css_class("mp-track-table");
            if let Some(sorter) = self.column_view.sorter() {
                self.sort_model.set_sorter(Some(&sorter));
            }
            let weak = obj.downgrade();
            self.column_view.connect_activate(move |_, position| {
                if let Some(list) = weak.upgrade() {
                    list.activate_row(position);
                }
            });
            let weak = obj.downgrade();
            context_menu::connect_keys(&*obj, move || {
                if let Some(list) = weak.upgrade() {
                    list.open_menu_for_focus();
                }
            });

            let scrolled = gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .vexpand(true)
                .child(&self.column_view)
                .build();
            self.stack.add_named(&scrolled, Some("list"));
            self.stack.add_named(&self.empty_page, Some("empty"));
            self.empty_page.set_icon_name(Some(EMPTY_ICON));
            self.empty_page.set_title("No Songs");
            self.empty_page.add_css_class("compact");
            // A breakpoint bin needs a minimum size of its own; this is the smallest
            // useful list.
            self.breakpoints.set_size_request(240, 160);
            self.breakpoints.set_child(Some(&self.stack));
            obj.set_child(Some(&self.breakpoints));

            let weak = obj.downgrade();
            self.sort_model.connect_items_changed(move |_, _, _, _| {
                if let Some(list) = weak.upgrade() {
                    list.update_empty_state();
                }
            });

            self.popover.set_has_arrow(false);
            self.popover.set_halign(gtk::Align::Start);
            self.popover.set_parent(&*obj);
        }

        fn dispose(&self) {
            self.popover.unparent();
        }
    }

    impl WidgetImpl for TrackList {
        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            self.parent_size_allocate(width, height, baseline);
            self.popover.present();
        }
    }

    impl BinImpl for TrackList {}
}

glib::wrapper! {
    /// See the [module documentation](self).
    pub struct TrackList(ObjectSubclass<imp::TrackList>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl TrackList {
    pub fn builder(ctx: &Rc<AppContext>) -> TrackListBuilder {
        TrackListBuilder {
            ctx: ctx.clone(),
            columns: vec![
                TrackColumn::Title,
                TrackColumn::Album,
                TrackColumn::Duration,
            ],
            covers: false,
            artists: true,
            sortable: true,
            scrolls: true,
            sections: None,
            menu_section: None,
            on_activate: None,
            on_reorder: None,
            empty: None,
        }
    }

    /// Shows a different model of [`TrackObject`]s, keeping columns and sorting.
    pub fn set_model(&self, model: Option<&impl IsA<gio::ListModel>>) {
        self.imp().sort_model.set_model(model);
        self.update_empty_state();
    }

    /// The tracks in display order (after sorting).
    pub fn model(&self) -> gio::ListModel {
        self.imp().sort_model.clone().upcast()
    }

    /// Track ids in display order, e.g. to play the list.
    pub fn track_ids(&self) -> Vec<TrackId> {
        track_ids_of(&self.imp().sort_model)
    }

    /// The selected tracks, in display order.
    pub fn selected_tracks(&self) -> Vec<TrackObject> {
        let imp = self.imp();
        let selection = imp.selection.selection();
        let Some((rest, first)) = gtk::BitsetIter::init_first(&selection) else {
            return Vec::new();
        };
        std::iter::once(first)
            .chain(rest)
            .filter_map(|position| imp.sort_model.item(position).and_downcast())
            .collect()
    }

    /// Positions of the selected rows in display order, e.g. to tell apart several
    /// rows of the same track in a playlist.
    pub fn selected_positions(&self) -> Vec<u32> {
        drag::selected_positions(&self.imp().selection.selection())
    }

    /// The underlying column view, for tweaks the builder doesn't cover.
    pub fn column_view(&self) -> &gtk::ColumnView {
        &self.imp().column_view
    }

    /// Scrolls so the row at `position` is visible and focused.
    pub fn scroll_to(&self, position: u32) {
        self.imp()
            .column_view
            .scroll_to(position, None, gtk::ListScrollFlags::FOCUS, None);
    }

    /// Changes what the list shows while it is empty, e.g. to explain that the library
    /// is still being scanned. With `busy`, a spinner replaces the icon. Both texts are
    /// plain text.
    pub fn set_empty_state(&self, title: &str, description: &str, busy: bool) {
        show_empty_state(&self.imp().empty_page, title, description, EMPTY_ICON, busy);
    }

    /// Plays the whole list starting at `position`.
    fn play_from(&self, position: u32) {
        self.ctx()
            .player
            .play_tracks(self.track_ids(), position as usize);
    }

    fn ctx(&self) -> &Rc<AppContext> {
        self.imp()
            .ctx
            .get()
            .expect("track lists are created through TrackList::builder")
    }

    fn activate_row(&self, position: u32) {
        match self.imp().on_activate.borrow().as_ref() {
            Some(handler) => handler(self, position),
            None => self.play_from(position),
        }
    }

    /// One selected row plays the list from there; several play just the selection.
    fn play_selection(&self) {
        let tracks = self.selected_tracks();
        match (tracks.len(), self.imp().menu_position.get()) {
            (0, _) => {}
            (1, Some(position)) => self.activate_row(position),
            _ => self
                .ctx()
                .player
                .play_tracks(track_actions::ids_of(&tracks), 0),
        }
    }

    /// The `tracks.*` actions of the context menu: playback and playlists act on the
    /// selection, "Go to" on the row the menu was opened on.
    fn install_menu_actions(&self, ctx: &Rc<AppContext>) {
        let list = self.downgrade();
        let target = move || {
            let list = list.upgrade()?;
            Some(MenuSongs {
                songs: list.selected_tracks(),
                opened_on: list.menu_track()?,
            })
        };
        let list = self.downgrade();
        let play = move || {
            if let Some(list) = list.upgrade() {
                list.play_selection();
            }
        };
        track_actions::install_actions(self, ctx, target, play);
    }

    /// The track the context menu was opened on.
    fn menu_track(&self) -> Option<TrackObject> {
        let position = self.imp().menu_position.get()?;
        self.imp().sort_model.item(position).and_downcast()
    }

    fn update_empty_state(&self) {
        let imp = self.imp();
        let empty = imp.sort_model.n_items() == 0;
        imp.stack
            .set_visible_child_name(if empty { "empty" } else { "list" });
    }

    /// Opens the context menu for the row at `position`, pointing at `(x, y)` in
    /// `widget`'s coordinates. Right-clicking outside the selection selects that row.
    fn open_menu_at(&self, position: u32, widget: &gtk::Widget, x: f64, y: f64) {
        let imp = self.imp();
        if !imp.selection.is_selected(position) {
            imp.selection.select_item(position, true);
        }
        imp.menu_position.set(Some(position));
        let point = widget
            .compute_point(self, &graphene::Point::new(x as f32, y as f32))
            .unwrap_or_else(|| graphene::Point::new(0.0, 0.0));
        self.popup_menu(gdk::Rectangle::new(
            point.x() as i32,
            point.y() as i32,
            1,
            1,
        ));
    }

    /// Keyboard (Menu key, Shift+F10): opens the menu under the focused row.
    fn open_menu_for_focus(&self) {
        let imp = self.imp();
        let selection = imp.selection.selection();
        let Some((_, first)) = gtk::BitsetIter::init_first(&selection) else {
            return;
        };
        imp.menu_position.set(Some(first));
        let anchor = self
            .root()
            .and_then(|root| root.focus())
            .filter(|focus| focus.is_ancestor(self))
            .and_then(|focus| focus.compute_bounds(self))
            .map_or_else(
                || gdk::Rectangle::new(24, 24, 1, 1),
                |bounds| {
                    gdk::Rectangle::new(
                        bounds.x() as i32 + 24,
                        (bounds.y() + bounds.height()) as i32,
                        1,
                        1,
                    )
                },
            );
        self.popup_menu(anchor);
    }

    fn popup_menu(&self, anchor: gdk::Rectangle) {
        let imp = self.imp();
        imp.popover.set_menu_model(Some(&self.context_menu()));
        imp.popover.set_pointing_to(Some(&anchor));
        imp.popover.popup();
    }

    /// The songs' menu, plus the list's own section if it has one.
    fn context_menu(&self) -> gio::Menu {
        let menu = track_actions::context_menu(self.ctx());
        if let Some(extra) = self.imp().menu_section.borrow().as_ref() {
            menu.append_section(None, extra);
        }
        menu
    }

    /// Sizes the list to fit every row instead of scrolling (see
    /// [`TrackListBuilder::scrolls`]).
    fn show_all_rows(&self) {
        let imp = self.imp();
        imp.stack.set_vhomogeneous(false);
        if let Some(scrolled) = imp
            .stack
            .child_by_name("list")
            .and_downcast::<gtk::ScrolledWindow>()
        {
            scrolled.set_vscrollbar_policy(gtk::PolicyType::Never);
            scrolled.set_propagate_natural_height(true);
        }
    }

    /// Hides columns as the list gets narrower (see [`TrackColumn::hidden_below`]).
    fn add_breakpoints(&self, columns: &[(TrackColumn, gtk::ColumnViewColumn)]) {
        let mut widths: Vec<u32> = columns
            .iter()
            .filter_map(|(column, _)| column.hidden_below())
            .collect();
        widths.sort_unstable_by(|a, b| b.cmp(a));
        widths.dedup();
        // Only the last matching breakpoint applies, so each one also hides the
        // columns of the wider breakpoints before it.
        for width in widths {
            let breakpoint = max_width_breakpoint(f64::from(width));
            for (column, view_column) in columns {
                if column.hidden_below().is_some_and(|limit| limit >= width) {
                    breakpoint.add_setter(view_column, "visible", Some(&false.to_value()));
                }
            }
            mark_narrow_while(&breakpoint, self);
            self.imp().breakpoints.add_breakpoint(breakpoint);
        }
    }

    fn add_column(
        &self,
        column: TrackColumn,
        options: CellOptions,
        sortable: bool,
    ) -> gtk::ColumnViewColumn {
        let factory = self.cell_factory(column, options);
        let view_column = gtk::ColumnViewColumn::new(Some(column.header()), Some(factory));
        match column.fixed_width() {
            Some(width) => view_column.set_fixed_width(width),
            None => {
                view_column.set_expand(true);
                view_column.set_resizable(true);
            }
        }
        if sortable {
            view_column.set_sorter(column.sorter().as_ref());
        }
        self.imp().column_view.append_column(&view_column);
        view_column
    }

    /// Creates the [`TrackCell`]s of `column`, with their menu and drag gestures.
    fn cell_factory(
        &self,
        column: TrackColumn,
        options: CellOptions,
    ) -> gtk::SignalListItemFactory {
        let factory = gtk::SignalListItemFactory::new();
        let cache = options.covers.then(|| self.ctx().covers.clone());
        let weak = self.downgrade();
        factory.connect_setup(move |_, item| {
            let Some(cell) = item.downcast_ref::<gtk::ColumnViewCell>() else {
                return;
            };
            let widget = TrackCell::new(column, cache.as_ref());
            if !options.artists {
                widget.hide_artist();
            }
            if options.numbered {
                widget.leave_playing_to_number();
            }
            cell.set_focusable(false);
            cell.set_child(Some(&widget));
            if column == TrackColumn::Position {
                let weak_widget = widget.downgrade();
                cell.connect_position_notify(move |cell| {
                    if let Some(widget) = weak_widget.upgrade() {
                        widget.set_position(cell.position());
                    }
                });
            }
            if let Some(list) = weak.upgrade() {
                list.attach_menu_gestures(&widget, cell);
                list.attach_drag_and_drop(&widget, cell);
            }
        });
        factory.connect_bind(|_, item| {
            let Some(cell) = item.downcast_ref::<gtk::ColumnViewCell>() else {
                return;
            };
            let widget = cell.child().and_downcast::<TrackCell>();
            let track = cell.item().and_downcast::<TrackObject>();
            if let (Some(widget), Some(track)) = (widget, track) {
                widget.bind(&track, cell.position());
            }
        });
        factory.connect_unbind(|_, item| {
            let widget = item
                .downcast_ref::<gtk::ColumnViewCell>()
                .and_then(|cell| cell.child())
                .and_downcast::<TrackCell>();
            if let Some(widget) = widget {
                widget.unbind();
            }
        });
        factory
    }

    fn attach_menu_gestures(&self, widget: &TrackCell, cell: &gtk::ColumnViewCell) {
        let list = self.downgrade();
        let cell = cell.downgrade();
        context_menu::connect_press(widget, move |widget, x, y| {
            if let (Some(list), Some(cell)) = (list.upgrade(), cell.upgrade()) {
                list.open_menu_at(cell.position(), widget, x, y);
            }
        });
    }
}

/// How the cells of every column are set up.
#[derive(Clone, Copy)]
struct CellOptions {
    covers: bool,
    artists: bool,
    /// A number column marks the playing song, so its cover doesn't have to as well.
    numbered: bool,
}

/// Configures a [`TrackList`]; see [`TrackList::builder`].
#[must_use]
pub struct TrackListBuilder {
    ctx: Rc<AppContext>,
    columns: Vec<TrackColumn>,
    covers: bool,
    artists: bool,
    sortable: bool,
    scrolls: bool,
    sections: Option<(gtk::Sorter, gtk::ListItemFactory)>,
    menu_section: Option<gio::MenuModel>,
    on_activate: Option<ActivateHandler>,
    on_reorder: Option<drag::ReorderHandler>,
    empty: Option<(String, String)>,
}

impl TrackListBuilder {
    /// The columns to show, left to right. Default: title, album, duration.
    pub fn columns(mut self, columns: &[TrackColumn]) -> Self {
        self.columns = columns.to_vec();
        self
    }

    /// Show a small cover next to each title. Default: off.
    pub fn covers(mut self, covers: bool) -> Self {
        self.covers = covers;
        self
    }

    /// Show each track's artist under its title. Default: on. Turn it off where every
    /// track is by the same artist, such as most album pages.
    pub fn artists(mut self, artists: bool) -> Self {
        self.artists = artists;
        self
    }

    /// Let clicking column headers sort the list. Default: on. Turn it off where the
    /// order itself matters, such as playlists and the queue.
    pub fn sortable(mut self, sortable: bool) -> Self {
        self.sortable = sortable;
        self
    }

    /// Whether the list scrolls by itself. Default: on. Turn it off for short lists
    /// inside a scrolling page (an album's tracks): every row is then laid out and
    /// the page scrolls instead, so put the page in a viewport with a natural
    /// vertical scroll policy.
    pub fn scrolls(mut self, scrolls: bool) -> Self {
        self.scrolls = scrolls;
        self
    }

    /// Groups rows into sections by `sorter`, each introduced by a header that
    /// `headers` creates from a `gtk::ListHeader`, e.g. the discs of an album. Rows
    /// should already be in section order.
    pub fn sections(
        mut self,
        sorter: &impl IsA<gtk::Sorter>,
        headers: &impl IsA<gtk::ListItemFactory>,
    ) -> Self {
        self.sections = Some((sorter.clone().upcast(), headers.clone().upcast()));
        self
    }

    /// Extra items appended to the context menu. Their actions should act on
    /// [`TrackList::selected_tracks`].
    pub fn menu_section(mut self, section: &impl IsA<gio::MenuModel>) -> Self {
        self.menu_section = Some(section.clone().upcast());
        self
    }

    /// Replaces what activating a row does (default: play the list from that row).
    /// The handler receives the row's position in display order.
    pub fn on_activate(mut self, handler: impl Fn(&TrackList, u32) + 'static) -> Self {
        self.on_activate = Some(Box::new(handler));
        self
    }

    /// Lets rows be dragged to a new place. The handler receives the dragged rows'
    /// positions and the position they were dropped before (the row count for the
    /// end); it should reorder the model. Use with [`Self::sortable`] off.
    pub fn on_reorder(mut self, handler: impl Fn(&TrackList, &[u32], u32) + 'static) -> Self {
        self.on_reorder = Some(Box::new(handler));
        self
    }

    /// What to show when the list is empty. Both texts are plain text, so they may
    /// contain names or search queries as they are.
    pub fn empty_state(mut self, title: &str, description: &str) -> Self {
        self.empty = Some((title.to_owned(), description.to_owned()));
        self
    }

    pub fn build(self, model: &impl IsA<gio::ListModel>) -> TrackList {
        let list: TrackList = glib::Object::new();
        let imp = list.imp();
        list.install_menu_actions(&self.ctx);
        assert!(
            imp.ctx.set(self.ctx).is_ok(),
            "a new list has no context yet"
        );
        imp.menu_section.replace(self.menu_section);
        imp.on_activate.replace(self.on_activate);
        if self.on_reorder.is_some() {
            list.add_css_class("mp-reorderable");
            list.enable_drag_scrolling();
        }
        imp.drag.on_reorder.replace(self.on_reorder);
        if let Some((title, description)) = self.empty {
            list.set_empty_state(&title, &description, false);
        }
        let options = CellOptions {
            covers: self.covers,
            artists: self.artists,
            numbered: self
                .columns
                .iter()
                .any(|column| matches!(column, TrackColumn::Number | TrackColumn::Position)),
        };
        let columns: Vec<_> = self
            .columns
            .into_iter()
            .map(|column| {
                let view_column = list.add_column(column, options, self.sortable);
                (column, view_column)
            })
            .collect();
        list.add_breakpoints(&columns);
        if let Some((sorter, headers)) = self.sections {
            imp.sort_model.set_section_sorter(Some(&sorter));
            imp.column_view.set_header_factory(Some(&headers));
        }
        if !self.scrolls {
            list.show_all_rows();
        }
        list.set_model(Some(model));
        list
    }
}
