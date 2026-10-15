//! A responsive grid of album or artist cards.
//!
//! ```ignore
//! let grid = CardGrid::builder(ctx, CardKind::Albums(AlbumCardDetail::Artist))
//!     .build(&ctx.store.albums());
//! ```
//!
//! Cards are recycled as the grid scrolls, so it stays smooth with thousands of
//! albums. They fill their column, so the grid has no ragged edge at any width.
//! Clicking a card (or Enter) opens its page; right-click, a long press, the Menu key
//! or Shift+F10 open its menu. An empty grid shows a status page instead (see
//! [`CardGrid::set_empty_state`]).

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gio, glib};

use super::actions::BrowseItem;
use super::card::{CARD_ART_MIN_SIZE, CardKind, bind_card, new_card};
use super::card_menu::CardMenu;
use crate::context::AppContext;
use crate::ui::pages::NARROW_PAGE_WIDTH;
use crate::ui::{max_width_breakpoint, set_css_class, show_empty_state};

/// The most columns a grid `width` pixels wide can fit: cards are never narrower
/// than their art. GTK keeps widgets ready for about 30 rows of `max-columns` cards,
/// so a generous fixed maximum would create hundreds of cards nobody sees.
fn max_columns(width: i32) -> u32 {
    (width / CARD_ART_MIN_SIZE).max(1).unsigned_abs()
}

mod imp {
    use super::*;

    #[derive(glib::Properties)]
    #[properties(wrapper_type = super::CardGrid)]
    pub struct CardGrid {
        pub(super) ctx: OnceCell<Rc<AppContext>>,
        pub(super) kind: Cell<CardKind>,
        /// The stack, or a breakpoint bin around it for grids that scroll.
        pub(super) child: RefCell<Option<gtk::Widget>>,
        pub(super) stack: gtk::Stack,
        pub(super) grid: gtk::GridView,
        pub(super) empty_page: adw::StatusPage,
        pub(super) menu: OnceCell<Rc<CardMenu>>,
        /// Slimmer gutters (`mp-narrow`) for narrow pages. Grids that scroll set this
        /// themselves; others follow their page (see [`CardGridBuilder::scrolls`]).
        #[property(get, set = Self::set_narrow, explicit_notify)]
        pub(super) narrow: Cell<bool>,
    }

    impl CardGrid {
        fn set_narrow(&self, narrow: bool) {
            if self.narrow.replace(narrow) != narrow {
                set_css_class(&*self.obj(), "mp-narrow", narrow);
                self.obj().notify_narrow();
            }
        }
    }

    impl Default for CardGrid {
        fn default() -> Self {
            Self {
                ctx: OnceCell::new(),
                kind: Cell::new(CardKind::Artists),
                child: RefCell::default(),
                stack: gtk::Stack::new(),
                grid: gtk::GridView::new(None::<gtk::NoSelection>, None::<gtk::ListItemFactory>),
                empty_page: adw::StatusPage::new(),
                menu: OnceCell::new(),
                narrow: Cell::new(false),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for CardGrid {
        const NAME: &'static str = "MpCardGrid";
        type Type = super::CardGrid;
        type ParentType = gtk::Widget;
    }

    #[glib::derived_properties]
    impl ObjectImpl for CardGrid {
        fn constructed(&self) {
            self.parent_constructed();
            self.grid.add_css_class("mp-card-grid");
            self.grid.set_single_click_activate(true);
            self.grid.set_tab_behavior(gtk::ListTabBehavior::Item);

            let obj = self.obj();
            obj.add_css_class("mp-cards");
            // Raised to what fits once the grid knows its width (see `max_columns`).
            self.grid.set_max_columns(1);
            self.empty_page.add_css_class("compact");
            // Sized by the visible child, so a short grid isn't padded to the
            // empty page's height.
            self.stack.set_vhomogeneous(false);
            self.stack.set_hhomogeneous(false);
            self.stack.add_named(&self.empty_page, Some("empty"));
        }

        fn dispose(&self) {
            if let Some(child) = self.child.take() {
                child.unparent();
            }
            if let Some(menu) = self.menu.get() {
                menu.unparent();
            }
        }
    }

    // A plain widget rather than an `adw::Bin`, whose layout manager would bypass
    // `size_allocate`, where the menu must be presented.
    impl WidgetImpl for CardGrid {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            self.child
                .borrow()
                .as_ref()
                .map_or(gtk::SizeRequestMode::ConstantSize, |child| {
                    child.request_mode()
                })
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            self.child
                .borrow()
                .as_ref()
                .map_or((0, 0, -1, -1), |child| child.measure(orientation, for_size))
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            self.grid.set_max_columns(max_columns(width));
            if let Some(child) = self.child.borrow().as_ref() {
                child.allocate(width, height, baseline, None);
            }
            if let Some(menu) = self.menu.get() {
                menu.present();
            }
        }
    }
}

glib::wrapper! {
    /// See the [module documentation](self).
    pub struct CardGrid(ObjectSubclass<imp::CardGrid>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl CardGrid {
    pub fn builder(ctx: &Rc<AppContext>, kind: CardKind) -> CardGridBuilder {
        CardGridBuilder {
            ctx: ctx.clone(),
            kind,
            scrolls: true,
        }
    }

    /// The underlying grid view, e.g. to set an accessible label.
    pub fn grid_view(&self) -> &gtk::GridView {
        &self.imp().grid
    }

    /// Changes what the grid shows while it is empty. With `busy`, a spinner
    /// replaces the icon. Both texts are plain text.
    pub fn set_empty_state(&self, title: &str, description: &str, busy: bool) {
        let imp = self.imp();
        let icon_name = imp.kind.get().empty_icon();
        show_empty_state(&imp.empty_page, title, description, icon_name, busy);
    }

    fn set_child(&self, child: &impl IsA<gtk::Widget>) {
        child.set_parent(self);
        self.imp().child.replace(Some(child.clone().upcast()));
    }

    fn update_empty_state(&self) {
        let imp = self.imp();
        let empty = imp.grid.model().is_none_or(|model| model.n_items() == 0);
        imp.stack
            .set_visible_child_name(if empty { "empty" } else { "grid" });
    }

    fn ctx(&self) -> &Rc<AppContext> {
        self.imp()
            .ctx
            .get()
            .expect("grids are created through CardGrid::builder")
    }

    fn menu(&self) -> &Rc<CardMenu> {
        self.imp()
            .menu
            .get()
            .expect("grids are created through CardGrid::builder")
    }

    /// Opens the page of the card at `position`.
    fn activate_card(&self, position: u32) {
        let item = self
            .imp()
            .grid
            .model()
            .and_then(|model| model.item(position))
            .and_then(|object| BrowseItem::from_object(&object));
        if let Some(item) = item {
            self.ctx().navigator.navigate(item.route());
        }
    }

    fn factory(&self, kind: CardKind) -> gtk::SignalListItemFactory {
        let factory = gtk::SignalListItemFactory::new();
        let ctx = self.ctx().clone();
        let grid = self.downgrade();
        factory.connect_setup(move |_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let card = new_card(&ctx, kind);
            if let Some(grid) = grid.upgrade() {
                grid.menu().attach(&card);
            }
            item.set_child(Some(&card));
        });
        factory.connect_bind(|_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            if let (Some(card), Some(object)) = (item.child(), item.item())
                && let Some(label) = bind_card(&card, &object)
            {
                item.set_accessible_label(&label);
            }
        });
        factory
    }
}

/// Configures a [`CardGrid`]; see [`CardGrid::builder`].
#[must_use]
pub struct CardGridBuilder {
    ctx: Rc<AppContext>,
    kind: CardKind,
    scrolls: bool,
}

impl CardGridBuilder {
    /// Whether the grid scrolls by itself. Default: on. Turn it off for grids inside
    /// a scrolling page (see [`super::DetailPage`]): every card is then created, so
    /// keep the model short and fill it with a [`super::GradualStore`]. The page
    /// should also set the grid's `narrow` property along with its own layout.
    pub fn scrolls(mut self, scrolls: bool) -> Self {
        self.scrolls = scrolls;
        self
    }

    /// Builds a grid of the items in `model`.
    pub fn build(self, model: &impl IsA<gio::ListModel>) -> CardGrid {
        let grid: CardGrid = glib::Object::new();
        let imp = grid.imp();
        assert!(
            imp.ctx.set(self.ctx.clone()).is_ok(),
            "a new grid has no context yet"
        );
        imp.kind.set(self.kind);
        imp.empty_page.set_icon_name(Some(self.kind.empty_icon()));
        imp.grid.set_factory(Some(&grid.factory(self.kind)));

        // Watching the grid's own wrapper rather than `model`, which may outlive it.
        let selection = gtk::NoSelection::new(Some(model.clone()));
        let weak = grid.downgrade();
        selection.connect_items_changed(move |_, _, _, _| {
            if let Some(grid) = weak.upgrade() {
                grid.update_empty_state();
            }
        });
        imp.grid.set_model(Some(&selection));
        let weak = grid.downgrade();
        imp.grid.connect_activate(move |_, position| {
            if let Some(grid) = weak.upgrade() {
                grid.activate_card(position);
            }
        });

        assert!(
            imp.menu.set(CardMenu::new(&grid, &self.ctx)).is_ok(),
            "a new grid has no menu yet"
        );

        if self.scrolls {
            let scrolled = gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .vexpand(true)
                .child(&imp.grid)
                .build();
            imp.stack.add_named(&scrolled, Some("grid"));
            grid.set_child(&narrow_breakpoint_bin(&grid, &imp.stack));
        } else {
            // A breakpoint bin would size the grid for its widest layout, leaving a
            // gap under it, so the page decides instead.
            imp.stack.add_named(&imp.grid, Some("grid"));
            grid.set_child(&imp.stack);
        }
        grid.update_empty_state();
        grid
    }
}

/// Wraps `child` so that `grid` turns narrow along with its width.
fn narrow_breakpoint_bin(grid: &CardGrid, child: &impl IsA<gtk::Widget>) -> adw::BreakpointBin {
    let breakpoint = max_width_breakpoint(NARROW_PAGE_WIDTH);
    breakpoint.add_setter(grid, "narrow", Some(&true.to_value()));
    // A breakpoint bin needs a minimum size of its own: one small card.
    let bin = adw::BreakpointBin::builder()
        .width_request(160)
        .height_request(120)
        .child(child)
        .build();
    bin.add_breakpoint(breakpoint);
    bin
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_columns_never_limits_what_fits() {
        assert_eq!(max_columns(0), 1);
        assert_eq!(max_columns(CARD_ART_MIN_SIZE - 1), 1);
        assert_eq!(max_columns(CARD_ART_MIN_SIZE * 5 + 20), 5);
    }
}
