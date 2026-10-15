//! A square that fills the width it is given by scaling a fixed-size child.

use std::cell::{Cell, RefCell};

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{glib, graphene, gsk};

/// How far the child may be enlarged to fill the width, e.g. two columns of cards on
/// a narrow page. Slight enough to stay sharp; beyond it the child is centred.
const MAX_SCALE: f32 = 1.25;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct ScaledSquare {
        pub(super) child: RefCell<Option<gtk::Widget>>,
        pub(super) min_size: Cell<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ScaledSquare {
        const NAME: &'static str = "MpScaledSquare";
        type Type = super::ScaledSquare;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for ScaledSquare {
        fn dispose(&self) {
            if let Some(child) = self.child.take() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for ScaledSquare {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let full = self.full_size();
            let side = match orientation {
                gtk::Orientation::Horizontal => {
                    return (self.min_size.get().min(full), full, -1, -1);
                }
                _ if for_size < 0 => full,
                _ => for_size.min(self.largest_size()),
            };
            (side, side, -1, -1)
        }

        fn size_allocate(&self, width: i32, height: i32, _baseline: i32) {
            let Some(child) = self.child.borrow().clone() else {
                return;
            };
            let full = self.full_size();
            if full <= 0 {
                return;
            }
            let scale = (width.min(height) as f32 / full as f32).min(MAX_SCALE);
            let offset = (width as f32 - full as f32 * scale) / 2.0;
            let transform = gsk::Transform::new()
                .translate(&graphene::Point::new(offset, 0.0))
                .scale(scale, scale);
            child.allocate(full, full, -1, Some(transform));
        }
    }

    impl ScaledSquare {
        /// The child's natural width.
        fn full_size(&self) -> i32 {
            self.child
                .borrow()
                .as_ref()
                .map_or(0, |child| child.measure(gtk::Orientation::Horizontal, -1).1)
        }

        fn largest_size(&self) -> i32 {
            (self.full_size() as f32 * MAX_SCALE) as i32
        }
    }
}

glib::wrapper! {
    /// Shows `child`, which has a fixed square size such as a [`crate::ui::widgets::CoverImage`],
    /// as wide as the available width, down to `min_size`. The child is scaled rather
    /// than resized, so its art is loaded once at one size and never reloaded.
    pub struct ScaledSquare(ObjectSubclass<imp::ScaledSquare>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ScaledSquare {
    pub fn new(child: &impl IsA<gtk::Widget>, min_size: i32) -> Self {
        let square: Self = glib::Object::new();
        let imp = square.imp();
        imp.min_size.set(min_size);
        child.set_parent(&square);
        imp.child.replace(Some(child.clone().upcast()));
        square
    }
}
