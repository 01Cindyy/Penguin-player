//! Artist cards: a round artist image with the name and how much music they have.

use std::cell::{OnceCell, RefCell};
use std::rc::{Rc, Weak};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;

use super::ArtistImage;
use super::actions::{self, BrowseItem};
use super::card::{CARD_ART_SIZE, card_art, card_label};
use crate::context::AppContext;
use crate::model::ArtistObject;
use crate::util::{count_label, song_count};

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct ArtistCard {
        pub(super) ctx: OnceCell<Weak<AppContext>>,
        pub(super) image: OnceCell<ArtistImage>,
        pub(super) play: gtk::Button,
        pub(super) name: gtk::Label,
        pub(super) stats: gtk::Label,
        pub(super) artist: RefCell<Option<ArtistObject>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ArtistCard {
        const NAME: &'static str = "MpArtistCard";
        type Type = super::ArtistCard;
        type ParentType = adw::Bin;
    }

    impl ObjectImpl for ArtistCard {}
    impl WidgetImpl for ArtistCard {}
    impl BinImpl for ArtistCard {}
}

glib::wrapper! {
    /// An artist in a grid (`mp-artist-card`): their round image, name and how much
    /// music they have. Rebound as grids scroll, like [`super::AlbumCard`].
    pub struct ArtistCard(ObjectSubclass<imp::ArtistCard>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ArtistCard {
    pub fn new(ctx: &Rc<AppContext>) -> Self {
        let card: Self = glib::Object::new();
        card.add_css_class("mp-artist-card");
        let imp = card.imp();
        imp.ctx
            .set(Rc::downgrade(ctx))
            .expect("a new card has no context yet");

        let image = ArtistImage::with_pixel_size(&ctx.covers, CARD_ART_SIZE);
        let art = card_art(&card, &image, &imp.play);
        imp.image.set(image).expect("a new card has no image yet");

        imp.name.add_css_class("mp-artist-name");
        imp.stats.add_css_class("mp-artist-stats");
        imp.stats.add_css_class("dim-label");
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&art);
        content.append(card_label(&imp.name, 0.5));
        content.append(card_label(&imp.stats, 0.5));
        card.set_child(Some(&content));

        imp.play.set_tooltip_text(Some("Play"));
        let weak = card.downgrade();
        imp.play.connect_clicked(move |_| {
            if let Some(card) = weak.upgrade() {
                card.play();
            }
        });
        card
    }

    /// Shows `artist`.
    pub fn bind(&self, artist: &ArtistObject) {
        let imp = self.imp();
        let name = artist.name();
        imp.image
            .get()
            .expect("cards are created with an image")
            .set_artist(&name, artist.cover());
        imp.name.set_label(&name);
        imp.name.set_tooltip_text(Some(&name));
        imp.stats.set_label(&artist_stats(
            artist.album_count() as usize,
            artist.track_count() as usize,
        ));
        imp.play
            .update_property(&[gtk::accessible::Property::Label(&format!("Play {name}"))]);
        imp.artist.replace(Some(artist.clone()));
    }

    /// The artist shown, if any.
    pub fn artist(&self) -> Option<ArtistObject> {
        self.imp().artist.borrow().clone()
    }

    fn play(&self) {
        let ctx = self.imp().ctx.get().and_then(Weak::upgrade);
        if let (Some(ctx), Some(artist)) = (ctx, self.artist()) {
            actions::play(&ctx, &BrowseItem::Artist(artist));
        }
    }
}

/// "3 albums · 27 songs".
pub fn artist_stats(albums: usize, songs: usize) -> String {
    format!(
        "{} · {}",
        count_label(albums, "album", "albums"),
        song_count(songs)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_pluralise() {
        assert_eq!(artist_stats(1, 1), "1 album · 1 song");
        assert_eq!(artist_stats(3, 1200), "3 albums · 1,200 songs");
    }
}
