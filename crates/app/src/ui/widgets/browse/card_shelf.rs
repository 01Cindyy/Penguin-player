//! One row of album or artist cards: as many as fit the width, like a shelf with a
//! "Show All" link next to it.
//!
//! The cards are the ones of [`super::CardGrid`], sized like the first row of one, and
//! behave the same: clicking one (or Enter) opens its page, and they have the same
//! play button and menu.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use gtk::subclass::prelude::*;

use super::card::{CardKind, bind_card, card_item, new_card};
use super::card_menu::CardMenu;
use crate::context::AppContext;

/// Space between cards in pixels, as between the columns of a [`super::CardGrid`].
const SPACING: i32 = 6;

mod imp {
    use super::*;

    #[derive(Default, glib::Properties)]
    #[properties(wrapper_type = super::CardShelf)]
    pub struct CardShelf {
        pub(super) ctx: OnceCell<Rc<AppContext>>,
        pub(super) kind: OnceCell<CardKind>,
        pub(super) menu: OnceCell<Rc<CardMenu>>,
        /// A button around each card. Created as needed and kept for later items.
        pub(super) buttons: RefCell<Vec<gtk::Button>>,
        /// How many of the buttons have an item to show.
        pub(super) used: Cell<usize>,
        /// How many cards fit and are shown.
        #[property(get)]
        pub(super) shown: Cell<u32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for CardShelf {
        const NAME: &'static str = "MpCardShelf";
        type Type = super::CardShelf;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("shelf");
            klass.set_accessible_role(gtk::AccessibleRole::List);
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for CardShelf {
        fn dispose(&self) {
            for button in self.buttons.take() {
                button.unparent();
            }
            if let Some(menu) = self.menu.get() {
                menu.unparent();
            }
        }
    }

    impl WidgetImpl for CardShelf {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        /// At least one card wide; naturally as wide as all cards together. As tall
        /// as the cards are at the width they get.
        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let buttons = self.buttons.borrow();
            let used = &buttons[..self.used.get()];
            if orientation == gtk::Orientation::Horizontal {
                let (minimum, natural) = used
                    .iter()
                    .map(|button| button.measure(orientation, -1))
                    .fold((0, -SPACING), |(minimum, natural), size| {
                        (minimum.max(size.0), natural + size.1 + SPACING)
                    });
                return (minimum, natural.max(minimum), -1, -1);
            }
            let card_width = match for_size {
                ..0 => -1,
                width => card_slots(used.len(), self.card_min_width(), width, SPACING)
                    .first()
                    .map_or(-1, |&(_, card_width)| card_width),
            };
            let (minimum, natural) = used
                .iter()
                .map(|button| button.measure(orientation, card_width))
                .fold((0, 0), |(minimum, natural), size| {
                    (minimum.max(size.0), natural.max(size.1))
                });
            (minimum, natural, -1, -1)
        }

        fn size_allocate(&self, width: i32, height: i32, _baseline: i32) {
            let buttons = self.buttons.borrow();
            let slots = card_slots(self.used.get(), self.card_min_width(), width, SPACING);
            for (index, button) in buttons.iter().enumerate() {
                let slot = slots.get(index).copied();
                button.set_child_visible(slot.is_some());
                if let Some((x, card_width)) = slot {
                    let allocation = gtk::Allocation::new(x, 0, card_width, height);
                    button.size_allocate(&allocation, -1);
                }
            }
            if let Some(menu) = self.menu.get() {
                menu.present();
            }
            let shown = slots.len() as u32;
            if self.shown.replace(shown) != shown {
                // Listeners may change other widgets' sizes, which mustn't happen in
                // the middle of size allocation; tell them at the start of the next frame.
                self.obj().add_tick_callback(|shelf, _| {
                    shelf.notify_shown();
                    glib::ControlFlow::Break
                });
            }
        }
    }

    impl CardShelf {
        /// The narrowest a card can be, which decides how many fit.
        fn card_min_width(&self) -> i32 {
            self.buttons.borrow()[..self.used.get()]
                .iter()
                .map(|button| button.measure(gtk::Orientation::Horizontal, -1).0)
                .max()
                .unwrap_or(0)
        }
    }
}

glib::wrapper! {
    /// See the [module documentation](self).
    pub struct CardShelf(ObjectSubclass<imp::CardShelf>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl CardShelf {
    pub fn new(ctx: &Rc<AppContext>, kind: CardKind) -> Self {
        let shelf: Self = glib::Object::new();
        shelf.add_css_class("mp-cards");
        let imp = shelf.imp();
        assert!(
            imp.ctx.set(ctx.clone()).is_ok(),
            "a new shelf has no context yet"
        );
        assert!(imp.kind.set(kind).is_ok(), "a new shelf has no kind yet");
        assert!(
            imp.menu.set(CardMenu::new(&shelf, ctx)).is_ok(),
            "a new shelf has no menu yet"
        );
        shelf
    }

    /// Shows `items`, albums or artists as the shelf's kind says, in order.
    pub fn set_items(&self, items: &[impl IsA<glib::Object>]) {
        let imp = self.imp();
        while imp.buttons.borrow().len() < items.len() {
            let button = self.new_card_button();
            imp.buttons.borrow_mut().push(button);
        }
        for (button, item) in imp.buttons.borrow().iter().zip(items) {
            let label = button
                .child()
                .and_then(|card| bind_card(&card, item.upcast_ref()));
            if let Some(label) = label {
                button.update_property(&[gtk::accessible::Property::Label(&label)]);
            }
        }
        imp.used.set(items.len());
        self.queue_resize();
    }

    /// A card in a flat button that opens the card's page.
    fn new_card_button(&self) -> gtk::Button {
        let imp = self.imp();
        let ctx = imp.ctx.get().expect("shelves are created with a context");
        let kind = *imp.kind.get().expect("shelves are created with a kind");
        let card = new_card(ctx, kind);
        self.menu().attach(&card);
        let button = gtk::Button::builder()
            .child(&card)
            .css_classes(["flat", "mp-card-button"])
            .build();
        let ctx = Rc::downgrade(ctx);
        button.connect_clicked(move |button| {
            let item = button.child().and_then(|card| card_item(&card));
            if let (Some(ctx), Some(item)) = (ctx.upgrade(), item) {
                ctx.navigator.navigate(item.route());
            }
        });
        button.set_parent(self);
        button
    }

    fn menu(&self) -> &Rc<CardMenu> {
        self.imp()
            .menu
            .get()
            .expect("shelves are created with a menu")
    }
}

/// Where the first of `count` cards go and how wide they are, as `(x, width)`, in
/// `available` pixels: like the columns of a [`super::CardGrid`], as many as fit at
/// `min_width` share the width equally. Fewer cards keep the columns' width.
fn card_slots(count: usize, min_width: i32, available: i32, spacing: i32) -> Vec<(i32, i32)> {
    let columns = ((available + spacing) / (min_width + spacing).max(1)).max(1);
    let shown = count.min(columns as usize) as i32;
    (0..shown)
        .map(|column| {
            let x = column * (available + spacing) / columns;
            let next = (column + 1) * (available + spacing) / columns;
            (x, next - x - spacing)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_many_columns_as_fit_share_the_width() {
        assert_eq!(card_slots(5, 100, 320, 12), [(0, 154), (166, 154)]);
        assert_eq!(
            card_slots(5, 100, 336, 12),
            [(0, 104), (116, 104), (232, 104)]
        );
        assert_eq!(
            card_slots(5, 100, 340, 12),
            [(0, 105), (117, 105), (234, 106)]
        );
    }

    #[test]
    fn fewer_cards_keep_the_column_width() {
        assert_eq!(card_slots(1, 100, 336, 12), [(0, 104)]);
        assert!(card_slots(0, 100, 336, 12).is_empty());
    }

    #[test]
    fn at_least_one_card_shows_however_narrow() {
        assert_eq!(card_slots(3, 100, 50, 12), [(0, 50)]);
    }
}
