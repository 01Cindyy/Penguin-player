//! A small pill naming a track's format, e.g. "FLAC", styled by its [`QualityTier`].

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;
use mp_core::Track;

use crate::ui::set_css_class;

/// How good a track's audio is, in the three tiers that badges, meters and the
/// Formats page colour alike. Ordered from plain to gold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum QualityTier {
    /// Lossy formats, and formats the scanner didn't recognise.
    Lossy,
    /// Lossless at CD resolution or below.
    Lossless,
    /// Lossless above CD resolution (see [`Track::is_hi_res`]).
    HiRes,
}

impl QualityTier {
    pub const BEST_FIRST: [QualityTier; 3] = [
        QualityTier::HiRes,
        QualityTier::Lossless,
        QualityTier::Lossy,
    ];

    pub fn of(track: &Track) -> Self {
        if track.is_hi_res() {
            QualityTier::HiRes
        } else if track.format.is_lossless() {
            QualityTier::Lossless
        } else {
            QualityTier::Lossy
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            QualityTier::HiRes => "Hi-Res",
            QualityTier::Lossless => "CD Quality",
            QualityTier::Lossy => "Lossy",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            QualityTier::HiRes => "Lossless above 16-bit or 48 kHz",
            QualityTier::Lossless => "Lossless up to 16-bit and 48 kHz",
            QualityTier::Lossy => "Compressed, such as MP3 or AAC",
        }
    }

    /// `mp-hires`, `mp-lossless` or `mp-lossy`, for badges and meters.
    pub fn css_class(self) -> &'static str {
        match self {
            QualityTier::HiRes => "mp-hires",
            QualityTier::Lossless => "mp-lossless",
            QualityTier::Lossy => "mp-lossy",
        }
    }
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct FormatBadge {
        pub(super) label: gtk::Label,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FormatBadge {
        const NAME: &'static str = "MpFormatBadge";
        type Type = super::FormatBadge;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("format-badge");
        }
    }

    impl ObjectImpl for FormatBadge {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.add_css_class("mp-format-badge");
            obj.set_valign(gtk::Align::Center);
            obj.set_halign(gtk::Align::Start);
            self.label.set_single_line_mode(true);
            obj.set_child(Some(&self.label));
        }
    }

    impl WidgetImpl for FormatBadge {}
    impl BinImpl for FormatBadge {}
}

glib::wrapper! {
    /// Format label styled by its tier (see [`QualityTier::css_class`]), with the
    /// quality (e.g. "24-bit / 96 kHz") as its tooltip.
    pub struct FormatBadge(ObjectSubclass<imp::FormatBadge>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for FormatBadge {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl FormatBadge {
    pub fn new() -> Self {
        Self::default()
    }

    /// Shows `track`'s format, or nothing for `None`.
    pub fn set_track(&self, track: Option<&Track>) {
        let Some(track) = track else {
            self.set_visible(false);
            return;
        };
        self.set_visible(true);
        self.imp().label.set_label(track.format.label());
        let tier = QualityTier::of(track);
        for other in QualityTier::BEST_FIRST {
            set_css_class(self, other.css_class(), other == tier);
        }

        let quality = track.quality_label();
        let tooltip = match (tier, quality.is_empty()) {
            (QualityTier::HiRes, false) => format!("Hi-Res · {quality}"),
            (QualityTier::HiRes, true) => "Hi-Res".to_owned(),
            (_, false) => quality,
            (_, true) => track.format.label().to_owned(),
        };
        self.set_tooltip_text(Some(&tooltip));
    }
}
