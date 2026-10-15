//! The items of a page section that lists only its first few until expanded.

use std::cell::{Cell, Ref, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};

use super::GradualStore;
use crate::util::count_label;

/// A section's items (see [`super::DetailPage::add_section`]): the first `limit`
/// are listed until the section's "Show All" button is pressed, so pages with
/// prolific artists stay short and quick to open.
pub struct CollapsibleItems<T> {
    all: RefCell<Vec<T>>,
    limit: usize,
    /// What the items are called, singular and plural, for the button's tooltip.
    noun: (&'static str, &'static str),
    expanded: Cell<bool>,
    listed: GradualStore,
    toggle: gtk::Button,
}

impl<T: IsA<glib::Object>> CollapsibleItems<T> {
    /// Lists up to `limit` items until expanded. The list fills on the frames of
    /// `page` (see [`GradualStore`]).
    pub fn new(
        page: &impl IsA<gtk::Widget>,
        limit: usize,
        noun: (&'static str, &'static str),
    ) -> Rc<Self> {
        let toggle = gtk::Button::builder()
            .css_classes(["flat", "mp-show-all"])
            .visible(false)
            .build();
        let items = Rc::new(Self {
            all: RefCell::default(),
            limit,
            noun,
            expanded: Cell::new(false),
            listed: GradualStore::new::<T>(page),
            toggle,
        });
        let weak = Rc::downgrade(&items);
        items.toggle.connect_clicked(move |_| {
            if let Some(items) = weak.upgrade() {
                items.expanded.set(!items.expanded.get());
                items.update();
            }
        });
        items
    }

    /// The listed items, for the section's view.
    pub fn model(&self) -> &gio::ListStore {
        self.listed.model()
    }

    /// "Show All" / "Show Less", for the end of the section's heading. Hidden while
    /// every item is listed anyway.
    pub fn toggle_button(&self) -> &gtk::Button {
        &self.toggle
    }

    /// Every item, listed or not.
    pub fn all(&self) -> Ref<'_, [T]> {
        Ref::map(self.all.borrow(), Vec::as_slice)
    }

    pub fn is_empty(&self) -> bool {
        self.all.borrow().is_empty()
    }

    pub fn set(&self, items: Vec<T>) {
        self.all.replace(items);
        self.update();
    }

    fn update(&self) {
        let all = self.all.borrow();
        let expanded = self.expanded.get();
        self.listed
            .set(&all[..listed_count(all.len(), self.limit, expanded)]);

        self.toggle.set_visible(all.len() > self.limit);
        let (label, tooltip) = toggle_text(expanded, all.len(), self.limit, self.noun);
        self.toggle.set_label(label);
        self.toggle.set_tooltip_text(Some(&tooltip));
    }
}

/// How many of `total` items are listed.
fn listed_count(total: usize, limit: usize, expanded: bool) -> usize {
    if expanded { total } else { total.min(limit) }
}

/// The toggle button's label and tooltip.
fn toggle_text(
    expanded: bool,
    total: usize,
    limit: usize,
    (singular, plural): (&str, &str),
) -> (&'static str, String) {
    if expanded {
        let first = count_label(limit, singular, plural);
        ("Show Less", format!("List only the first {first}"))
    } else {
        let all = count_label(total, singular, plural);
        ("Show All", format!("List all {all}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_the_first_items_until_expanded() {
        assert_eq!(listed_count(30, 10, false), 10);
        assert_eq!(listed_count(30, 10, true), 30);
        assert_eq!(listed_count(4, 10, false), 4);
    }

    #[test]
    fn toggle_describes_what_it_does() {
        let noun = ("song", "songs");
        assert_eq!(
            toggle_text(false, 1200, 10, noun),
            ("Show All", "List all 1,200 songs".to_owned())
        );
        assert_eq!(
            toggle_text(true, 1200, 10, noun),
            ("Show Less", "List only the first 10 songs".to_owned())
        );
    }
}
