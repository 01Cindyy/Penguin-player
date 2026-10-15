use std::cell::OnceCell;

use gtk::glib;
use gtk::glib::subclass::prelude::*;
use mp_core::{AlbumKey, Artist, CoverId, SearchQuery};

mod imp {
    use std::marker::PhantomData;

    use gtk::glib::prelude::*;

    use super::*;

    #[derive(Default, glib::Properties)]
    #[properties(wrapper_type = super::ArtistObject)]
    pub struct ArtistObject {
        pub(super) artist: OnceCell<Artist>,

        #[property(get = |o: &Self| o.artist().name.clone(), type = String)]
        name: PhantomData<String>,
        #[property(get = |o: &Self| o.artist().albums.len() as u32, type = u32)]
        album_count: PhantomData<u32>,
        #[property(get = |o: &Self| o.artist().track_count as u32, type = u32)]
        track_count: PhantomData<u32>,
    }

    impl ArtistObject {
        pub(super) fn artist(&self) -> &Artist {
            self.artist
                .get()
                .expect("ArtistObject::new always sets the artist")
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ArtistObject {
        const NAME: &'static str = "MpArtistObject";
        type Type = super::ArtistObject;
    }

    #[glib::derived_properties]
    impl ObjectImpl for ArtistObject {}
}

glib::wrapper! {
    /// An album artist derived from the library's tracks (see [`mp_core::collection`]).
    pub struct ArtistObject(ObjectSubclass<imp::ArtistObject>);
}

impl ArtistObject {
    pub fn new(artist: Artist) -> Self {
        let object: Self = glib::Object::new();
        object
            .imp()
            .artist
            .set(artist)
            .expect("a freshly created ArtistObject has no artist yet");
        object
    }

    pub fn artist(&self) -> &Artist {
        self.imp().artist()
    }

    /// The normalised name used in [`crate::ui::navigation::Route::Artist`].
    pub fn key(&self) -> &str {
        &self.artist().key
    }

    /// Albums oldest first.
    pub fn album_keys(&self) -> &[AlbumKey] {
        &self.artist().albums
    }

    pub fn cover(&self) -> Option<&CoverId> {
        self.artist().cover.as_ref()
    }

    /// Normalised name that search queries match against: the artist's key.
    pub fn search_text(&self) -> &str {
        &self.artist().key
    }

    pub fn matches(&self, query: &SearchQuery) -> bool {
        query.is_empty() || query.matches(self.search_text())
    }
}
