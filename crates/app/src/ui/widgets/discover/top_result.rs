//! The "Top Result" card on the search page: the best match shown large, with a play
//! button. Activating the card opens the result (or plays it, for a song).

use std::cell::{Cell, OnceCell, RefCell};
use std::sync::OnceLock;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;
use gtk::glib::subclass::Signal;
use mp_core::CoverId;

use crate::model::{AlbumObject, ArtistObject, TrackObject};
use crate::services::covers::CoverCache;
use crate::ui::icons;
use crate::ui::set_css_class;
use crate::ui::widgets::CoverImage;
use crate::ui::widgets::browse::ArtistImage;
use crate::util::{count_label, song_count};

/// Side of the card's art in pixels.
const ART_SIZE: i32 = 112;
/// Room kept free for the play button in the card's corner, in pixels.
const PLAY_BUTTON_SPACE: i32 = 64;

/// The best match for a search.
#[derive(Debug, Clone, PartialEq)]
pub enum TopResult {
    Song(TrackObject),
    Album(AlbumObject),
    Artist(ArtistObject),
}

impl TopResult {
    pub fn kind_label(&self) -> &'static str {
        match self {
            TopResult::Song(_) => "Song",
            TopResult::Album(_) => "Album",
            TopResult::Artist(_) => "Artist",
        }
    }

    pub fn title(&self) -> String {
        match self {
            TopResult::Song(track) => track.title(),
            TopResult::Album(album) => album.title(),
            TopResult::Artist(artist) => artist.name(),
        }
    }

    /// Secondary line: who made it, or how much there is.
    pub fn details(&self) -> String {
        match self {
            TopResult::Song(track) => format!("{} · {}", track.artist(), track.album()),
            TopResult::Album(album) => match album.year() {
                0 => album.artist(),
                year => format!("{} · {year}", album.artist()),
            },
            TopResult::Artist(artist) => format!(
                "{} · {}",
                count_label(artist.album_count() as usize, "album", "albums"),
                song_count(artist.track_count() as usize)
            ),
        }
    }

    fn cover(&self) -> Option<&CoverId> {
        match self {
            TopResult::Song(track) => track.track().cover.as_ref(),
            TopResult::Album(album) => album.cover(),
            TopResult::Artist(artist) => artist.cover(),
        }
    }
}

mod imp {
    use super::*;

    #[derive(Default, glib::Properties)]
    #[properties(wrapper_type = super::TopResultCard)]
    pub struct TopResultCard {
        pub(super) result: RefCell<Option<TopResult>>,
        /// Shows `cover`, or `artist_image` for artists.
        pub(super) art: adw::Bin,
        pub(super) cover: OnceCell<CoverImage>,
        pub(super) artist_image: OnceCell<ArtistImage>,
        pub(super) card: gtk::Button,
        pub(super) layout: gtk::Box,
        pub(super) text: gtk::Box,
        pub(super) title: gtk::Label,
        pub(super) kind: gtk::Label,
        pub(super) details: gtk::Label,
        pub(super) play: gtk::Button,
        /// Art beside the text rather than above it, for full-width cards.
        #[property(get, set = Self::set_horizontal, explicit_notify)]
        pub(super) horizontal: Cell<bool>,
    }

    impl TopResultCard {
        fn set_horizontal(&self, horizontal: bool) {
            if self.horizontal.replace(horizontal) != horizontal {
                self.obj().update_layout();
                self.obj().notify_horizontal();
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TopResultCard {
        const NAME: &'static str = "MpTopResultCard";
        type Type = super::TopResultCard;
        type ParentType = adw::Bin;
    }

    #[glib::derived_properties]
    impl ObjectImpl for TopResultCard {
        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![
                    Signal::builder("open").build(),
                    Signal::builder("play").build(),
                ]
            })
        }
    }

    impl WidgetImpl for TopResultCard {}
    impl BinImpl for TopResultCard {}
}

glib::wrapper! {
    /// The top result card (`mp-top-result`). Emits `open` when the card is activated
    /// and `play` from its play button (`mp-top-result-play`).
    pub struct TopResultCard(ObjectSubclass<imp::TopResultCard>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl TopResultCard {
    pub fn new(covers: &CoverCache) -> Self {
        let card: Self = glib::Object::new();
        let imp = card.imp();
        let cover = CoverImage::with_pixel_size(covers, ART_SIZE);
        assert!(imp.cover.set(cover).is_ok(), "a new card has no cover yet");
        let artist_image = ArtistImage::with_pixel_size(covers, ART_SIZE);
        assert!(
            imp.artist_image.set(artist_image).is_ok(),
            "a new card has no artist image yet"
        );
        imp.art.set_halign(gtk::Align::Start);
        card.build_text();
        imp.layout.append(&imp.art);
        imp.layout.append(&imp.text);
        card.build_buttons();
        card.update_layout();
        card
    }

    /// The title above the result's kind and details.
    fn build_text(&self) {
        let imp = self.imp();
        imp.title.set_xalign(0.0);
        imp.title.set_wrap(true);
        imp.title.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        imp.title.set_lines(2);
        imp.title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        // Natural widths stay small so the card doesn't claim the songs' room; the
        // labels still use all the width the card gets.
        imp.title.set_max_width_chars(12);
        imp.title.add_css_class("title-1");
        imp.title.add_css_class("mp-top-result-title");
        imp.kind.set_valign(gtk::Align::Center);
        imp.kind.add_css_class("mp-kind-pill");
        imp.details.set_xalign(0.0);
        imp.details.set_hexpand(true);
        imp.details.set_ellipsize(gtk::pango::EllipsizeMode::End);
        imp.details.set_max_width_chars(12);
        imp.details.add_css_class("dim-label");
        imp.details.add_css_class("mp-top-result-details");

        let meta = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        meta.append(&imp.kind);
        meta.append(&imp.details);
        imp.text.set_orientation(gtk::Orientation::Vertical);
        imp.text.set_spacing(6);
        imp.text.set_margin_end(PLAY_BUTTON_SPACE);
        imp.text.append(&imp.title);
        imp.text.append(&meta);
    }

    /// The card is a button that opens the result, with a play button in its corner.
    fn build_buttons(&self) {
        let imp = self.imp();
        imp.card.set_child(Some(&imp.layout));
        imp.card.add_css_class("mp-top-result");
        self.emit_on_click(&imp.card, "open");

        imp.play.set_icon_name(icons::PLAY);
        imp.play.set_halign(gtk::Align::End);
        imp.play.set_valign(gtk::Align::End);
        imp.play.add_css_class("mp-top-result-play");
        self.emit_on_click(&imp.play, "play");

        let overlay = gtk::Overlay::builder().child(&imp.card).build();
        overlay.add_overlay(&imp.play);
        self.set_child(Some(&overlay));
    }

    fn emit_on_click(&self, button: &gtk::Button, signal: &'static str) {
        let weak = self.downgrade();
        button.connect_clicked(move |_| {
            if let Some(card) = weak.upgrade() {
                card.emit_by_name::<()>(signal, &[]);
            }
        });
    }

    pub fn result(&self) -> Option<TopResult> {
        self.imp().result.borrow().clone()
    }

    pub fn set_result(&self, result: &TopResult) {
        let imp = self.imp();
        let title = result.title();
        let details = result.details();
        imp.title.set_label(&title);
        imp.kind.set_label(result.kind_label());
        imp.details.set_label(&details);
        self.show_art(result);

        let kind = result.kind_label();
        imp.card
            .set_tooltip_text(Some(&format!("{title}\n{details}")));
        imp.card
            .update_property(&[gtk::accessible::Property::Label(&format!(
                "Top result: {kind} “{title}”, {details}"
            ))]);
        let play_label = format!("Play “{title}”");
        imp.play.set_tooltip_text(Some(&play_label));
        imp.play
            .update_property(&[gtk::accessible::Property::Label(&play_label)]);
        imp.result.replace(Some(result.clone()));
    }

    pub fn connect_open<F: Fn(&Self) + 'static>(&self, f: F) -> glib::SignalHandlerId {
        self.connect_closure(
            "open",
            false,
            glib::closure_local!(move |card: &Self| f(card)),
        )
    }

    pub fn connect_play<F: Fn(&Self) + 'static>(&self, f: F) -> glib::SignalHandlerId {
        self.connect_closure(
            "play",
            false,
            glib::closure_local!(move |card: &Self| f(card)),
        )
    }

    fn update_layout(&self) {
        let imp = self.imp();
        let horizontal = imp.horizontal.get();
        imp.layout.set_orientation(if horizontal {
            gtk::Orientation::Horizontal
        } else {
            gtk::Orientation::Vertical
        });
        imp.layout.set_spacing(18);
        // Stacked, the text sits at the bottom of the card beside the play button;
        // side by side, it is centred next to the art.
        imp.text.set_valign(if horizontal {
            gtk::Align::Center
        } else {
            gtk::Align::End
        });
        imp.text.set_vexpand(!horizontal);
        set_css_class(self, "mp-horizontal", horizontal);
    }

    /// Album art, or for an artist the same round image as their cards.
    fn show_art(&self, result: &TopResult) {
        let imp = self.imp();
        let art: gtk::Widget = match result {
            TopResult::Artist(artist) => {
                let image = imp.artist_image.get().expect("cards are created with art");
                image.set_artist(&artist.name(), artist.cover());
                image.clone().upcast()
            }
            _ => {
                let cover = imp.cover.get().expect("cards are created with art");
                cover.set_cover(result.cover());
                cover.clone().upcast()
            }
        };
        if imp.art.child().as_ref() != Some(&art) {
            imp.art.set_child(Some(&art));
        }
    }
}
