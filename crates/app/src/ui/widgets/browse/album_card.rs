//! An album card: the cover with a play button on hover, the title and one line of
//! detail under it.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::{Rc, Weak};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;

use super::actions::{self, BrowseItem};
use super::card::{CARD_ART_SIZE, card_art, card_label};
use crate::context::AppContext;
use crate::model::AlbumObject;
use crate::ui::widgets::CoverImage;
use crate::util::song_count;

/// What an [`AlbumCard`] shows under the title.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AlbumCardDetail {
    /// The album artist, for grids that mix artists.
    #[default]
    Artist,
    /// The release year, for grids of one artist's albums.
    Year,
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct AlbumCard {
        pub(super) ctx: OnceCell<Weak<AppContext>>,
        pub(super) cover: OnceCell<CoverImage>,
        pub(super) play: gtk::Button,
        pub(super) title: gtk::Label,
        pub(super) detail: gtk::Label,
        pub(super) detail_kind: Cell<AlbumCardDetail>,
        pub(super) album: RefCell<Option<AlbumObject>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for AlbumCard {
        const NAME: &'static str = "MpAlbumCard";
        type Type = super::AlbumCard;
        type ParentType = adw::Bin;
    }

    impl ObjectImpl for AlbumCard {}
    impl WidgetImpl for AlbumCard {}
    impl BinImpl for AlbumCard {}
}

glib::wrapper! {
    /// An album in a grid (`mp-album-card`, with `mp-album-title` and
    /// `mp-album-artist`). Cards are created once and rebound as grids scroll, so
    /// [`Self::bind`] only updates labels and the cover.
    pub struct AlbumCard(ObjectSubclass<imp::AlbumCard>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl AlbumCard {
    pub fn new(ctx: &Rc<AppContext>, detail: AlbumCardDetail) -> Self {
        let card: Self = glib::Object::new();
        card.add_css_class("mp-album-card");
        let imp = card.imp();
        imp.ctx
            .set(Rc::downgrade(ctx))
            .expect("a new card has no context yet");
        imp.detail_kind.set(detail);

        let cover = CoverImage::with_pixel_size(&ctx.covers, CARD_ART_SIZE);
        cover.add_css_class("mp-cover-medium");
        let art = card_art(&card, &cover, &imp.play);
        imp.cover.set(cover).expect("a new card has no cover yet");

        imp.title.add_css_class("mp-album-title");
        imp.detail.add_css_class("mp-album-artist");
        imp.detail.add_css_class("dim-label");
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&art);
        content.append(card_label(&imp.title, 0.0));
        content.append(card_label(&imp.detail, 0.0));
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

    /// Shows `album`.
    pub fn bind(&self, album: &AlbumObject) {
        let imp = self.imp();
        let title = album.title();
        imp.cover
            .get()
            .expect("cards are created with a cover")
            .set_cover(album.cover());
        imp.title.set_label(&title);
        imp.title.set_tooltip_text(Some(&title));
        imp.detail.set_label(&match imp.detail_kind.get() {
            AlbumCardDetail::Artist => album.artist(),
            AlbumCardDetail::Year => match album.year() {
                0 => song_count(album.track_count() as usize),
                year => year.to_string(),
            },
        });
        imp.play
            .update_property(&[gtk::accessible::Property::Label(&format!("Play {title}"))]);
        imp.album.replace(Some(album.clone()));
    }

    /// The album shown, if any.
    pub fn album(&self) -> Option<AlbumObject> {
        self.imp().album.borrow().clone()
    }

    fn play(&self) {
        let ctx = self.imp().ctx.get().and_then(Weak::upgrade);
        if let (Some(ctx), Some(album)) = (ctx, self.album()) {
            actions::play(&ctx, &BrowseItem::Album(album));
        }
    }
}
