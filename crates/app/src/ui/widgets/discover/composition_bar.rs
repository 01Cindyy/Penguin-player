//! A slim bar split into segments proportional to their weights, such as how many
//! songs each format has.

use std::cell::RefCell;

use adw::prelude::*;
use gtk::glib;
use gtk::subclass::prelude::*;

/// Space between segments, in pixels.
const GAP: i32 = 2;
/// The narrowest a segment gets, so tiny shares stay visible.
const MIN_SEGMENT_WIDTH: i32 = 4;

/// One part of the bar.
pub struct Segment {
    pub weight: u64,
    /// Colours the segment, e.g. a codec class.
    pub css_class: &'static str,
    pub tooltip: String,
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct CompositionBar {
        pub(super) segments: RefCell<Vec<(gtk::Widget, u64)>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for CompositionBar {
        const NAME: &'static str = "MpCompositionBar";
        type Type = super::CompositionBar;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("compositionbar");
            klass.set_accessible_role(gtk::AccessibleRole::Img);
        }
    }

    impl ObjectImpl for CompositionBar {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.add_css_class("mp-composition-bar");
            obj.set_overflow(gtk::Overflow::Hidden);
        }

        fn dispose(&self) {
            for (widget, _) in self.segments.take() {
                widget.unparent();
            }
        }
    }

    impl WidgetImpl for CompositionBar {
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let segments = self.segments.borrow();
            if orientation == gtk::Orientation::Horizontal {
                let count = segments.len() as i32;
                let minimum = (count * (MIN_SEGMENT_WIDTH + GAP) - GAP).max(0);
                return (minimum, minimum, -1, -1);
            }
            let height = segments
                .iter()
                .map(|(widget, _)| widget.measure(orientation, -1).0)
                .max()
                .unwrap_or(0);
            (height, height, -1, -1)
        }

        fn size_allocate(&self, width: i32, height: i32, _baseline: i32) {
            let segments = self.segments.borrow();
            let weights: Vec<u64> = segments.iter().map(|(_, weight)| *weight).collect();
            let widths = segment_widths(width, &weights, GAP, MIN_SEGMENT_WIDTH);
            let mut x = 0;
            for ((widget, _), segment_width) in segments.iter().zip(widths) {
                widget.size_allocate(&gtk::Allocation::new(x, 0, segment_width, height), -1);
                x += segment_width + GAP;
            }
        }
    }
}

glib::wrapper! {
    /// A proportional bar (`mp-composition-bar`); each segment is an
    /// `mp-composition-segment` with its own class and tooltip.
    pub struct CompositionBar(ObjectSubclass<imp::CompositionBar>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for CompositionBar {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl CompositionBar {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces the segments; empty ones are left out. `description` is what screen
    /// readers announce in place of the picture.
    pub fn set_segments(&self, segments: Vec<Segment>, description: &str) {
        let imp = self.imp();
        for (widget, _) in imp.segments.take() {
            widget.unparent();
        }
        let widgets = segments
            .into_iter()
            .filter(|segment| segment.weight > 0)
            .map(|segment| {
                let widget: gtk::Widget = adw::Bin::builder()
                    .css_classes(["mp-composition-segment", segment.css_class])
                    .tooltip_text(&segment.tooltip)
                    .build()
                    .upcast();
                widget.set_parent(self);
                (widget, segment.weight)
            })
            .collect();
        imp.segments.replace(widgets);
        self.update_property(&[gtk::accessible::Property::Label(description)]);
        self.queue_resize();
    }
}

/// Splits `available` pixels between segments in proportion to `weights`, leaving
/// `gap` pixels between neighbours. Every segment gets at least `min_width` (taken
/// from the widest ones) when there is room, and the widths always fill the space.
fn segment_widths(available: i32, weights: &[u64], gap: i32, min_width: i32) -> Vec<i32> {
    let count = weights.len() as i32;
    let total: u64 = weights.iter().sum();
    if count == 0 || total == 0 {
        return vec![0; weights.len()];
    }
    let usable = (available - gap * (count - 1)).max(0);

    // Largest remainder: floor every share, then give the leftover pixels to the
    // segments that lost the most to rounding.
    let exact: Vec<f64> = weights
        .iter()
        .map(|&weight| f64::from(usable) * weight as f64 / total as f64)
        .collect();
    let mut widths: Vec<i32> = exact.iter().map(|share| share.floor() as i32).collect();
    let mut by_remainder: Vec<usize> = (0..widths.len()).collect();
    by_remainder.sort_by(|&a, &b| {
        let remainder = |i: usize| exact[i] - exact[i].floor();
        remainder(b).total_cmp(&remainder(a))
    });
    let leftover = usable - widths.iter().sum::<i32>();
    for &index in by_remainder.iter().take(leftover.max(0) as usize) {
        widths[index] += 1;
    }

    if usable >= min_width * count {
        for index in 0..widths.len() {
            while widths[index] < min_width {
                let widest = (0..widths.len())
                    .max_by_key(|&i| widths[i])
                    .expect("there is at least one segment");
                widths[widest] -= 1;
                widths[index] += 1;
            }
        }
    }
    widths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths_fill_the_bar() {
        let widths = segment_widths(100, &[1, 1, 1], 2, 4);
        assert_eq!(widths.iter().sum::<i32>(), 96);
        assert!(widths.iter().all(|&w| w == 32));

        let widths = segment_widths(203, &[700, 290, 10], 2, 4);
        assert_eq!(widths.iter().sum::<i32>(), 199);
        assert_eq!(widths[2], 4);
        assert!(widths[0] > widths[1]);
    }

    #[test]
    fn degenerate_inputs() {
        assert!(segment_widths(100, &[], 2, 4).is_empty());
        assert_eq!(segment_widths(100, &[0, 0], 2, 4), vec![0, 0]);
        assert_eq!(segment_widths(5, &[1, 1, 1], 2, 4).iter().sum::<i32>(), 1);
    }
}
