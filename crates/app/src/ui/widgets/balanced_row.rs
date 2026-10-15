//! A row of up to three widgets whose middle one stays centred: the two sides are
//! always given the same width, and the middle gets a fair share before they grow.
//!
//! Unlike [`gtk::CenterBox`], a long start widget (such as a song title) is cut short
//! rather than pushing the middle off centre.

use std::cell::{Cell, RefCell};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct BalancedRow {
        pub(super) start: RefCell<Option<gtk::Widget>>,
        pub(super) center: RefCell<Option<gtk::Widget>>,
        pub(super) end: RefCell<Option<gtk::Widget>>,
        /// Width the middle gets before the sides grow past their minimum.
        pub(super) center_width: Cell<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for BalancedRow {
        const NAME: &'static str = "MpBalancedRow";
        type Type = super::BalancedRow;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for BalancedRow {
        fn dispose(&self) {
            for slot in [&self.start, &self.center, &self.end] {
                if let Some(child) = slot.take() {
                    child.unparent();
                }
            }
        }
    }

    impl WidgetImpl for BalancedRow {
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let size = |slot: &RefCell<Option<gtk::Widget>>| {
                slot.borrow().as_ref().map_or((0, 0), |child| {
                    let (minimum, natural, ..) = child.measure(orientation, -1);
                    (minimum, natural)
                })
            };
            let (start, center, end) = (size(&self.start), size(&self.center), size(&self.end));
            let (minimum, natural) = match orientation {
                gtk::Orientation::Horizontal if self.center.borrow().is_some() => (
                    center.0 + 2 * start.0.max(end.0),
                    center.1.max(self.center_width.get()) + 2 * start.1.max(end.1),
                ),
                gtk::Orientation::Horizontal => (start.0 + end.0, start.1 + end.1),
                _ => (
                    start.0.max(center.0).max(end.0),
                    start.1.max(center.1).max(end.1),
                ),
            };
            (minimum, natural, -1, -1)
        }

        fn size_allocate(&self, width: i32, height: i32, _baseline: i32) {
            let measure = |slot: &RefCell<Option<gtk::Widget>>| {
                slot.borrow().as_ref().map_or((0, 0), |child| {
                    let (minimum, natural, ..) = child.measure(gtk::Orientation::Horizontal, -1);
                    (minimum, natural)
                })
            };
            let (start, end) = (measure(&self.start), measure(&self.end));
            let (start_width, center_width, end_width) = match self.center.borrow().as_ref() {
                Some(center) => {
                    let (center_minimum, ..) = center.measure(gtk::Orientation::Horizontal, -1);
                    let center_width =
                        split(width, center_minimum, self.center_width.get(), start, end);
                    let side = (width - center_width) / 2;
                    (side, width - 2 * side, side)
                }
                // Without a middle, the end keeps its natural width if it can and the
                // start takes the rest.
                None => {
                    let end_width = end.1.min(width - start.0).max(end.0);
                    (width - end_width, 0, end_width)
                }
            };

            let rtl = self.obj().direction() == gtk::TextDirection::Rtl;
            let place = |slot: &RefCell<Option<gtk::Widget>>, x: i32, child_width: i32| {
                if let Some(child) = slot.borrow().as_ref() {
                    let x = if rtl { width - x - child_width } else { x };
                    let position = gtk::graphene::Point::new(x as f32, 0.0);
                    child.allocate(
                        child_width,
                        height,
                        -1,
                        Some(gtk::gsk::Transform::new().translate(&position)),
                    );
                }
            };
            place(&self.start, 0, start_width);
            place(&self.center, start_width, center_width);
            place(&self.end, width - end_width, end_width);
        }
    }
}

glib::wrapper! {
    /// See the [module documentation](self).
    pub struct BalancedRow(ObjectSubclass<imp::BalancedRow>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl BalancedRow {
    /// The middle widget gets up to `center_width` pixels before the sides grow past
    /// their minimum.
    pub fn new(center_width: i32) -> Self {
        let row: Self = glib::Object::new();
        row.imp().center_width.set(center_width);
        row
    }

    pub fn set_start_widget(&self, widget: Option<&impl IsA<gtk::Widget>>) {
        self.replace(&self.imp().start, widget);
    }

    pub fn set_center_widget(&self, widget: Option<&impl IsA<gtk::Widget>>) {
        self.replace(&self.imp().center, widget);
    }

    pub fn set_end_widget(&self, widget: Option<&impl IsA<gtk::Widget>>) {
        self.replace(&self.imp().end, widget);
    }

    fn replace(&self, slot: &RefCell<Option<gtk::Widget>>, widget: Option<&impl IsA<gtk::Widget>>) {
        if let Some(old) = slot.take() {
            old.unparent();
        }
        if let Some(widget) = widget {
            // Children are kept in start, middle, end order for keyboard focus.
            let imp = self.imp();
            let next = [&imp.start, &imp.center, &imp.end]
                .into_iter()
                .skip_while(|other| !std::ptr::eq(*other, slot))
                .skip(1)
                .find_map(|other| other.borrow().clone());
            widget.insert_before(self, next.as_ref());
            slot.replace(Some(widget.clone().upcast()));
        }
        self.queue_resize();
    }
}

/// How wide the middle is in a row `width` wide: as wide as the sides' natural widths
/// leave, but at least `wanted` (or as close to it as the sides' minimums allow), and
/// never less than its own `minimum`. Sides are `(minimum, natural)`.
fn split(width: i32, minimum: i32, wanted: i32, start: (i32, i32), end: (i32, i32)) -> i32 {
    let side_minimum = start.0.max(end.0);
    let side_natural = start.1.max(end.1);
    let fair = wanted.min(width - 2 * side_minimum);
    (width - 2 * side_natural).max(fair).max(minimum)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_middle_gets_its_share_before_the_sides_grow() {
        // Sides fit at their natural width: the middle takes the rest.
        assert_eq!(split(1200, 300, 520, (180, 250), (200, 230)), 700);
        // A long title wants 330: it gives way so the middle keeps 520.
        assert_eq!(split(1024, 300, 520, (180, 330), (200, 230)), 520);
        // Narrower still: the sides shrink to their minimum first.
        assert_eq!(split(820, 300, 520, (180, 330), (200, 230)), 420);
        // The middle never goes below its own minimum.
        assert_eq!(split(600, 300, 520, (180, 330), (200, 230)), 300);
    }
}
