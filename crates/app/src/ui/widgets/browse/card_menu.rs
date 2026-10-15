//! The context menu of album and artist cards, shared by [`super::CardGrid`] and
//! [`super::CardShelf`]. It opens like every context menu (see
//! [`crate::ui::context_menu`]), under the card.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use adw::prelude::*;
use gtk::{gdk, gio, glib, graphene};

use super::actions::{self, BrowseItem};
use super::card::card_item;
use crate::context::AppContext;
use crate::ui::context_menu;

pub(super) struct CardMenu {
    ctx: Weak<AppContext>,
    host: glib::WeakRef<gtk::Widget>,
    popover: gtk::PopoverMenu,
    /// The album or artist the menu was opened for, which the `browse.*` actions use.
    item: Rc<RefCell<Option<BrowseItem>>>,
}

impl CardMenu {
    /// A menu for the cards inside `host`. The host must call [`Self::present`] from
    /// its `size_allocate` and [`Self::unparent`] when it is disposed.
    pub(super) fn new(host: &impl IsA<gtk::Widget>, ctx: &Rc<AppContext>) -> Rc<Self> {
        let popover = gtk::PopoverMenu::from_model(None::<&gio::MenuModel>);
        popover.set_has_arrow(false);
        popover.set_halign(gtk::Align::Start);
        popover.set_parent(host);
        let item: Rc<RefCell<Option<BrowseItem>>> = Rc::default();
        let target = item.clone();
        actions::install_actions(host, ctx, move || target.borrow().clone());
        let menu = Rc::new(Self {
            ctx: Rc::downgrade(ctx),
            host: host.upcast_ref::<gtk::Widget>().downgrade(),
            popover,
            item,
        });
        let weak = Rc::downgrade(&menu);
        context_menu::connect_keys(host, move || {
            if let Some(menu) = weak.upgrade() {
                menu.open_for_focus();
            }
        });
        menu
    }

    /// Opens the menu where `card` is right-clicked or long-pressed.
    pub(super) fn attach(self: &Rc<Self>, card: &impl IsA<gtk::Widget>) {
        let menu = Rc::downgrade(self);
        context_menu::connect_press(card, move |card, x, y| {
            if let Some(menu) = menu.upgrade() {
                menu.open(card, &graphene::Point::new(x as f32, y as f32));
            }
        });
    }

    /// Opens the menu in the middle of the focused card, if the focus is on one.
    fn open_for_focus(&self) {
        let Some(host) = self.host.upgrade() else {
            return;
        };
        let Some(focus) = host
            .root()
            .and_then(|root| root.focus())
            .filter(|focus| focus.is_ancestor(&host))
        else {
            return;
        };
        // The focus sits on the grid item or button that holds the card.
        let Some(card) = focus.first_child() else {
            return;
        };
        let middle = graphene::Point::new(card.width() as f32 / 2.0, card.height() as f32 / 2.0);
        self.open(&card, &middle);
    }

    pub(super) fn present(&self) {
        self.popover.present();
    }

    pub(super) fn unparent(&self) {
        self.popover.unparent();
    }

    /// Opens the menu for `card`, pointing at `point` in the card's coordinates.
    fn open(&self, card: &gtk::Widget, point: &graphene::Point) {
        let (Some(ctx), Some(host), Some(item)) =
            (self.ctx.upgrade(), self.host.upgrade(), card_item(card))
        else {
            return;
        };
        let point = card
            .compute_point(&host, point)
            .unwrap_or_else(|| graphene::Point::new(0.0, 0.0));
        let anchor = gdk::Rectangle::new(point.x() as i32, point.y() as i32, 1, 1);
        self.popover
            .set_menu_model(Some(&actions::card_menu_model(&ctx, item.kind())));
        self.item.replace(Some(item));
        self.popover.set_pointing_to(Some(&anchor));
        self.popover.popup();
    }
}
