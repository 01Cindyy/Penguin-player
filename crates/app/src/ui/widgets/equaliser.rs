//! The mark of the playing song: three small bars that bounce while music plays and hold
//! still while it is paused (see `.mp-equaliser` in style.css).

use gtk::prelude::*;

/// A new equaliser, in the colour of its surroundings.
pub fn equaliser() -> gtk::Box {
    let bars = gtk::Box::builder()
        .spacing(2)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .css_classes(["mp-equaliser"])
        .accessible_role(gtk::AccessibleRole::Img)
        .tooltip_text("Now playing")
        .build();
    bars.update_property(&[gtk::accessible::Property::Label("Now playing")]);
    for _ in 0..3 {
        bars.append(
            &gtk::Box::builder()
                .css_classes(["mp-equaliser-bar"])
                .build(),
        );
    }
    bars
}
