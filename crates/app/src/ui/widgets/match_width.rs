//! Keeps a child as wide as another widget, whose width can change with the window.

use std::cell::{Cell, RefCell};

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk};

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct MatchWidth {
        pub(super) child: RefCell<Option<gtk::Widget>>,
        pub(super) source: RefCell<Option<gtk::Widget>>,
        /// The source's width when the child was last sized to it.
        pub(super) matched_to: Cell<i32>,
        /// Checks the source after each layout (see `realize`).
        pub(super) layout_handler: RefCell<Option<(gdk::FrameClock, glib::SignalHandlerId)>>,
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
        /// GTK sizes this again only when its own size changes, but the source can change
        /// alone, as a cover does when the window gets shorter. So after each layout it
        /// checks the source, and if that changed, has itself sized again. GTK lays out
        /// again before drawing, so the child follows in the same frame.
        fn realize(&self) {
            self.parent_realize();
            let obj = self.obj();
            let Some(clock) = obj.frame_clock() else {
                return;
            };
            let widget = obj.downgrade();
            let handler = clock.connect_layout(move |_| {
                if let Some(widget) = widget.upgrade() {
                    widget.imp().follow_source();
                }
            });
            self.layout_handler.replace(Some((clock, handler)));
        }

        fn unrealize(&self) {
            if let Some((clock, handler)) = self.layout_handler.take() {
                clock.disconnect(handler);
            }
            self.parent_unrealize();
        }

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
            self.matched_to.set(source);
            let (minimum, ..) = child.measure(gtk::Orientation::Horizontal, -1);
            let matched = source.max(minimum).min(width);
            let offset = graphene::Point::new(((width - matched) / 2) as f32, 0.0);
            let transform = gsk::Transform::new().translate(&offset);
            child.allocate(matched, height, -1, Some(transform));
        }
    }

    impl MatchWidth {
        fn follow_source(&self) {
            let source = self.source.borrow().as_ref().map(|source| source.width());
            if source.is_some_and(|width| width != self.matched_to.get()) {
                self.obj().queue_allocate();
            }
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
