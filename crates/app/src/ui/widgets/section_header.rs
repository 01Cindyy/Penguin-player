//! The heading above a section of a page: its title and, at its end, an action such
//! as a "Show All" button.

use adw::prelude::*;

use crate::util::{count_label, format_count};

/// A section title (`mp-section-title`) and `action` at its end, in an
/// `mp-section-header` box.
pub fn section_heading(title: &str, action: Option<&impl IsA<gtk::Widget>>) -> gtk::Box {
    let label = gtk::Label::builder()
        .label(title)
        .xalign(0.0)
        .hexpand(true)
        .css_classes(["mp-section-title", "title-4"])
        .accessible_role(gtk::AccessibleRole::Heading)
        .build();
    let heading = gtk::Box::builder()
        .spacing(12)
        .css_classes(["mp-section-header"])
        .build();
    heading.append(&label);
    if let Some(action) = action {
        action.set_valign(gtk::Align::Center);
        heading.append(action);
    }
    heading
}

/// A [`section_heading`] whose "Show All" button (`mp-show-all`) appears while some
/// of the section's items are left out.
#[derive(Clone)]
pub struct SectionHeader {
    pub root: gtk::Box,
    pub show_all: gtk::Button,
    noun: (&'static str, &'static str),
}

impl SectionHeader {
    /// `noun` names one and several of the section's items, e.g. `("song", "songs")`.
    pub fn new(title: &str, noun: (&'static str, &'static str)) -> Self {
        let show_all = gtk::Button::builder()
            .css_classes(["flat", "mp-show-all"])
            .visible(false)
            .build();
        Self {
            root: section_heading(title, Some(&show_all)),
            show_all,
            noun,
        }
    }

    /// Offers "Show All {total}" while fewer than `total` items are on screen.
    pub fn set_show_all(&self, total: usize, shown: usize) {
        self.show_all.set_visible(total > shown);
        self.show_all
            .set_label(&format!("Show All {}", format_count(total)));
        let (singular, plural) = self.noun;
        let description = format!("Show all {}", count_label(total, singular, plural));
        self.show_all.set_tooltip_text(Some(&description));
    }
}
