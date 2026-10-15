//! The round artist image of artist cards and the artist page.

use std::cell::OnceCell;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;
use mp_core::CoverId;

use crate::services::covers::CoverCache;
use crate::ui::widgets::{CoverImage, CoverSize};

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct ArtistImage {
        pub(super) cover: OnceCell<CoverImage>,
        pub(super) avatar: OnceCell<adw::Avatar>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ArtistImage {
        const NAME: &'static str = "MpArtistImage";
        type Type = super::ArtistImage;
        type ParentType = adw::Bin;
    }

    impl ObjectImpl for ArtistImage {}
    impl WidgetImpl for ArtistImage {}
    impl BinImpl for ArtistImage {}
}

glib::wrapper! {
    /// A round artist image (`mp-artist-avatar`): the artist's first album cover, or
    /// their initials when no album has art.
    pub struct ArtistImage(ObjectSubclass<imp::ArtistImage>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ArtistImage {
    pub fn new(covers: &CoverCache, size: CoverSize) -> Self {
        Self::with_cover(CoverImage::new(covers, size), size.pixels())
    }

    /// An image of any size, for layouts the [`CoverSize`] presets don't fit.
    pub fn with_pixel_size(covers: &CoverCache, size: i32) -> Self {
        Self::with_cover(CoverImage::with_pixel_size(covers, size), size)
    }

    fn with_cover(cover: CoverImage, size: i32) -> Self {
        let image: Self = glib::Object::new();
        image.set_halign(gtk::Align::Center);
        image.set_valign(gtk::Align::Center);
        cover.add_css_class("mp-artist-avatar");
        let avatar = adw::Avatar::new(size, None, true);
        avatar.add_css_class("mp-artist-avatar");
        let imp = image.imp();
        imp.cover.set(cover).expect("a new image has no cover yet");
        imp.avatar
            .set(avatar)
            .expect("a new image has no avatar yet");
        image
    }

    /// Changes the width and height, e.g. for narrow pages.
    pub fn set_pixel_size(&self, size: i32) {
        let imp = self.imp();
        imp.cover.get().expect("set in new").set_pixel_size(size);
        imp.avatar.get().expect("set in new").set_size(size);
    }

    pub fn set_artist(&self, name: &str, cover: Option<&CoverId>) {
        let imp = self.imp();
        match cover {
            Some(cover) => {
                let image = imp.cover.get().expect("set in new");
                image.set_cover(Some(cover));
                image.update_property(&[gtk::accessible::Property::Label(name)]);
                self.show(image);
            }
            None => {
                let avatar = imp.avatar.get().expect("set in new");
                avatar.set_text(Some(name));
                self.show(avatar);
            }
        }
    }

    fn show(&self, child: &impl IsA<gtk::Widget>) {
        if self.child().as_ref() != Some(child.upcast_ref()) {
            self.set_child(Some(child));
        }
    }
}
