//! Keeps a child as wide as another widget, whose width can change with the window.

use std::cell::RefCell;

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{glib, graphene, gsk};

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct MatchWidth {
        pub(super) child: RefCell<Option<gtk::Widget>>,
        pub(super) source: RefCell<Option<gtk::Widget>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MatchWidth {
        const NAME: &'static str = "MpMatchWidth";
        type Type = super::MatchWidth;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for MatchWidth {
        fn dispose(&self) {
            if let Some(child) = self.child.take() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for MatchWidth {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            self.child
                .borrow()
                .as_ref()
                .map_or((0, 0, -1, -1), |child| {
                    let (minimum, natural, ..) = child.measure(orientation, for_size);
                    (minimum, natural, -1, -1)
                })
        }

        /// Runs after the source's own allocation: it comes first in their parent.
        fn size_allocate(&self, width: i32, height: i32, _baseline: i32) {
            let Some(child) = self.child.borrow().clone() else {
                return;
            };
            let source = self
                .source
                .borrow()
                .as_ref()
                .map_or(width, |source| source.width());
            let (minimum, ..) = child.measure(gtk::Orientation::Horizontal, -1);
            let matched = source.max(minimum).min(width);
            let offset = graphene::Point::new(((width - matched) / 2) as f32, 0.0);
            let transform = gsk::Transform::new().translate(&offset);
            child.allocate(matched, height, -1, Some(transform));
        }
    }
}

glib::wrapper! {
    /// Shows `child` centred and as wide as `source`, which must come before it in their
    /// parent, unless it doesn't fit in that width.
    pub struct MatchWidth(ObjectSubclass<imp::MatchWidth>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl MatchWidth {
    pub fn new(child: &impl IsA<gtk::Widget>, source: &impl IsA<gtk::Widget>) -> Self {
        let widget: Self = glib::Object::new();
        child.set_parent(&widget);
        let imp = widget.imp();
        imp.child.replace(Some(child.clone().upcast()));
        imp.source.replace(Some(source.clone().upcast()));
        widget
    }
}
