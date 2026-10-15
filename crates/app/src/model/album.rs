use std::cell::OnceCell;

use gtk::glib;
use gtk::glib::subclass::prelude::*;
use mp_core::search::normalize;
use mp_core::{Album, AlbumKey, CoverId, SearchQuery, TrackId};

mod imp {
    use std::marker::PhantomData;

    use gtk::glib::prelude::*;

    use super::*;

    #[derive(Default, glib::Properties)]
    #[properties(wrapper_type = super::AlbumObject)]
    pub struct AlbumObject {
        pub(super) album: OnceCell<Album>,
        pub(super) search_text: OnceCell<String>,

        #[property(get = |o: &Self| o.album().title.clone(), type = String)]
        title: PhantomData<String>,
        #[property(get = |o: &Self| o.album().artist.clone(), type = String)]
        artist: PhantomData<String>,
        /// Release year, 0 when unknown.
        #[property(get = |o: &Self| o.album().year.unwrap_or(0), type = u32)]
        year: PhantomData<u32>,
        #[property(get = |o: &Self| o.album().tracks.len() as u32, type = u32)]
        track_count: PhantomData<u32>,
        #[property(get = |o: &Self| o.album().duration_ms, type = u64)]
        duration_ms: PhantomData<u64>,
        /// Distinct format labels joined with " · ", e.g. "FLAC · MP3".
        #[property(get = |o: &Self| o.format_summary(), type = String)]
        formats: PhantomData<String>,
    }

    impl AlbumObject {
        pub(super) fn album(&self) -> &Album {
            self.album
                .get()
                .expect("AlbumObject::new always sets the album")
        }

        fn format_summary(&self) -> String {
            let labels: Vec<&str> = self.album().formats.iter().map(|f| f.label()).collect();
            labels.join(" · ")
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for AlbumObject {
        const NAME: &'static str = "MpAlbumObject";
        type Type = super::AlbumObject;
    }

    #[glib::derived_properties]
    impl ObjectImpl for AlbumObject {}
}

glib::wrapper! {
    /// An album derived from the library's tracks (see [`mp_core::collection`]).
    pub struct AlbumObject(ObjectSubclass<imp::AlbumObject>);
}

impl AlbumObject {
    pub fn new(album: Album) -> Self {
        let object: Self = glib::Object::new();
        object
            .imp()
            .album
            .set(album)
            .expect("a freshly created AlbumObject has no album yet");
        object
    }

    /// An album whose search text (see [`album_search_text`]) was already prepared,
    /// e.g. on a worker thread.
    pub fn with_search_text(album: Album, search_text: String) -> Self {
        let object = Self::new(album);
        object
            .imp()
            .search_text
            .set(search_text)
            .expect("a freshly created AlbumObject has no search text yet");
        object
    }

    pub fn album(&self) -> &Album {
        self.imp().album()
    }

    pub fn key(&self) -> &AlbumKey {
        &self.album().key
    }

    pub fn cover(&self) -> Option<&CoverId> {
        self.album().cover.as_ref()
    }

    /// Track ids in album order (disc, track number, title).
    pub fn track_ids(&self) -> &[TrackId] {
        &self.album().tracks
    }

    /// Normalised text that search queries match against, computed on first use
    /// unless it was prepared with the album.
    pub fn search_text(&self) -> &str {
        self.imp()
            .search_text
            .get_or_init(|| album_search_text(self.album()))
    }

    pub fn matches(&self, query: &SearchQuery) -> bool {
        query.is_empty() || query.matches(self.search_text())
    }
}

/// The album's title and artist, normalised for matching searches.
pub fn album_search_text(album: &Album) -> String {
    normalize(&format!("{} {}", album.title, album.artist))
}
