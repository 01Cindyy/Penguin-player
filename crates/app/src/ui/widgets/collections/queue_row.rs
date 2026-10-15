//! One row of the queue panel's "Up Next" list.

use std::cell::OnceCell;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;
use mp_core::util::format_duration;

use crate::model::TrackObject;
use crate::services::covers::CoverCache;
use crate::ui::icons;
use crate::ui::widgets::SongHeading;

/// The remove button's size; the duration's slot, which it shares, is at least as big.
const BUTTON_SIZE: i32 = 28;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct QueueRow {
        pub(super) heading: OnceCell<SongHeading>,
        pub(super) duration: gtk::Label,
        pub(super) remove: gtk::Button,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for QueueRow {
        const NAME: &'static str = "MpQueueRow";
        type Type = super::QueueRow;
        type ParentType = adw::Bin;
    }

    impl ObjectImpl for QueueRow {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().add_css_class("mp-queue-row");
            self.duration.set_xalign(1.0);
            self.duration.set_css_classes(&[
                "mp-track-duration",
                "numeric",
                "caption",
                "dim-label",
            ]);
            self.remove.set_icon_name(icons::REMOVE);
            self.remove.set_halign(gtk::Align::End);
            self.remove.set_valign(gtk::Align::Center);
            self.remove
                .set_css_classes(&["flat", "circular", "mp-queue-remove"]);
            self.remove.set_tooltip_text(Some("Remove from Queue"));
            self.remove
                .update_property(&[gtk::accessible::Property::Label("Remove from Queue")]);
        }
    }

    impl WidgetImpl for QueueRow {}
    impl BinImpl for QueueRow {}
}

glib::wrapper! {
    /// A [`SongHeading`] and the duration, which gives way to a remove button
    /// while the row is hovered or has the keyboard focus (`mp-queue-row`,
    /// `mp-queue-remove`).
    pub struct QueueRow(ObjectSubclass<imp::QueueRow>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl QueueRow {
    pub fn new(covers: &CoverCache) -> Self {
        let row: Self = glib::Object::new();
        let imp = row.imp();
        let heading = SongHeading::new(Some(covers));
        let end = gtk::Overlay::builder()
            .child(&imp.duration)
            .width_request(BUTTON_SIZE)
            .height_request(BUTTON_SIZE)
            .valign(gtk::Align::Center)
            .build();
        end.add_overlay(&imp.remove);

        let layout = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        layout.append(&heading.root);
        layout.append(&end);
        row.set_child(Some(&layout));
        assert!(imp.heading.set(heading).is_ok(), "set once in new");
        row
    }

    /// Shows `track`. Rows are recycled, so this only updates labels and the cover.
    pub fn set_track(&self, track: &TrackObject) {
        let imp = self.imp();
        if let Some(heading) = imp.heading.get() {
            heading.set_track(track);
        }
        imp.duration
            .set_label(&format_duration(track.track().duration()));
    }

    pub fn remove_button(&self) -> &gtk::Button {
        &self.imp().remove
    }
}
