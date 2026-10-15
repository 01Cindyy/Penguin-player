//! A number with a caption under it, such as "1,234" over "Songs".

use adw::prelude::*;

/// A figure (`mp-figure`): its value in `mp-figure-value`, the caption dimmed below.
pub struct Figure {
    pub root: gtk::Box,
    value: gtk::Label,
}

impl Figure {
    pub fn new(caption: &str) -> Self {
        // Both labels shrink with ellipsis, so the figure never widens its container.
        let value = gtk::Label::builder()
            .xalign(0.0)
            .max_width_chars(1)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["mp-figure-value", "heading", "numeric"])
            .build();
        let caption = gtk::Label::builder()
            .label(caption)
            .xalign(0.0)
            .max_width_chars(1)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["caption", "dim-label"])
            .build();
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .css_classes(["mp-figure"])
            .build();
        root.append(&value);
        root.append(&caption);
        Self { root, value }
    }

    pub fn set_value(&self, value: &str) {
        self.value.set_label(value);
        self.value.set_tooltip_text(Some(value));
    }

    /// The value's tooltip, e.g. the exact figure behind a rounded value.
    pub fn set_detail(&self, detail: &str) {
        self.value.set_tooltip_text(Some(detail));
    }
}
