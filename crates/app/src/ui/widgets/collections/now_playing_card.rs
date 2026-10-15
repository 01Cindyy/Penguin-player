//! The card at the top of the queue panel showing the current track.

use std::cell::{Cell, OnceCell};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;

use crate::model::TrackObject;
use crate::services::covers::CoverCache;
use crate::ui::widgets::{CoverImage, CoverSize, FormatBadge};

mod imp {
    use super::*;

    #[derive(Default, glib::Properties)]
    #[properties(wrapper_type = super::NowPlayingCard)]
    pub struct NowPlayingCard {
        pub(super) cover: OnceCell<CoverImage>,
        pub(super) title: gtk::Label,
        pub(super) artist: gtk::Label,
        pub(super) badge: FormatBadge,
        pub(super) details: gtk::Label,
        /// Shows bit depth, sample rate and bit rate next to the format badge.
        #[property(get, set = Self::set_quality_details, explicit_notify)]
        quality_details: Cell<bool>,
    }

    impl NowPlayingCard {
        fn set_quality_details(&self, enabled: bool) {
            if self.quality_details.replace(enabled) != enabled {
                self.obj().update_details();
                self.obj().notify_quality_details();
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for NowPlayingCard {
        const NAME: &'static str = "MpNowPlayingCard";
        type Type = super::NowPlayingCard;
        type ParentType = adw::Bin;
    }

    #[glib::derived_properties]
    impl ObjectImpl for NowPlayingCard {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().add_css_class("mp-now-playing-card");
            for (label, classes) in [
                (&self.title, &["mp-now-title", "heading"][..]),
                (&self.artist, &["mp-now-artist", "dim-label"]),
                (
                    &self.details,
                    &["mp-now-quality-details", "caption", "dim-label", "numeric"],
                ),
            ] {
                label.set_xalign(0.0);
                label.set_ellipsize(gtk::pango::EllipsizeMode::End);
                label.set_single_line_mode(true);
                label.set_css_classes(classes);
            }
            self.details.set_visible(false);
        }
    }

    impl WidgetImpl for NowPlayingCard {}
    impl BinImpl for NowPlayingCard {}
}

glib::wrapper! {
    /// Cover, title, artist and quality of the current track (`mp-now-playing-card`):
    /// the format, and with `quality-details` everything else known about it.
    pub struct NowPlayingCard(ObjectSubclass<imp::NowPlayingCard>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl NowPlayingCard {
    pub fn new(covers: &CoverCache) -> Self {
        let card: Self = glib::Object::new();
        let imp = card.imp();
        let cover = CoverImage::new(covers, CoverSize::Small);

        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .valign(gtk::Align::Center)
            .spacing(2)
            .hexpand(true)
            .build();
        text.append(&imp.title);
        text.append(&imp.artist);
        let quality = gtk::Box::builder().spacing(6).margin_top(4).build();
        quality.append(&imp.badge);
        quality.append(&imp.details);
        text.append(&quality);
        let layout = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        layout.append(&cover);
        layout.append(&text);
        card.set_child(Some(&layout));
        assert!(imp.cover.set(cover).is_ok(), "set once in new");
        card
    }

    pub fn set_track(&self, track: Option<&TrackObject>) {
        let imp = self.imp();
        imp.title
            .set_label(&track.map(TrackObject::title).unwrap_or_default());
        imp.artist
            .set_label(&track.map(TrackObject::artist).unwrap_or_default());
        imp.badge.set_track(track.map(TrackObject::track));
        let details = track
            .map(|track| track.track().quality_details())
            .unwrap_or_default();
        imp.details.set_label(&details);
        imp.details.set_tooltip_text(Some(&details));
        self.update_details();
        if let Some(cover) = imp.cover.get() {
            cover.set_cover(track.and_then(|track| track.track().cover.as_ref()));
        }
    }

    fn update_details(&self) {
        let details = &self.imp().details;
        details.set_visible(self.quality_details() && !details.label().is_empty());
    }
}
