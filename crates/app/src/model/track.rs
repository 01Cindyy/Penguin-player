use std::cell::OnceCell;

use gtk::glib;
use gtk::glib::subclass::prelude::*;
use mp_core::{AlbumKey, SearchQuery, Track, TrackId};

mod imp {
    use std::cell::Cell;
    use std::marker::PhantomData;

    use gtk::glib::prelude::*;

    use super::*;

    #[derive(Default, glib::Properties)]
    #[properties(wrapper_type = super::TrackObject)]
    pub struct TrackObject {
        pub(super) track: OnceCell<Track>,
        pub(super) search_text: OnceCell<String>,

        #[property(get = |o: &Self| o.track().id.0, type = i64)]
        id: PhantomData<i64>,
        #[property(get = |o: &Self| o.track().title.clone(), type = String)]
        title: PhantomData<String>,
        /// Track artist, or "Unknown Artist".
        #[property(get = |o: &Self| o.track().display_artist().to_owned(), type = String)]
        artist: PhantomData<String>,
        /// Album title, or "Unknown Album".
        #[property(get = |o: &Self| o.track().display_album().to_owned(), type = String)]
        album: PhantomData<String>,
        /// The artist the album is filed under (album artist, falling back to the artist).
        #[property(get = |o: &Self| o.track().display_album_artist().to_owned(), type = String)]
        album_artist: PhantomData<String>,
        #[property(get = |o: &Self| o.track().genre.clone(), type = String)]
        genre: PhantomData<String>,
        /// Release year, 0 when unknown.
        #[property(get = |o: &Self| o.track().year.unwrap_or(0), type = u32)]
        year: PhantomData<u32>,
        /// Track number on its disc, 0 when unknown.
        #[property(get = |o: &Self| o.track().track_number.unwrap_or(0), type = u32)]
        track_number: PhantomData<u32>,
        /// Disc number, 1 when unknown.
        #[property(get = |o: &Self| o.track().disc_number.unwrap_or(1), type = u32)]
        disc_number: PhantomData<u32>,
        #[property(get = |o: &Self| o.track().duration_ms, type = u64)]
        duration_ms: PhantomData<u64>,
        /// Format label such as "FLAC".
        #[property(get = |o: &Self| o.track().format.label().to_owned(), type = String)]
        format: PhantomData<String>,
        /// Quality description such as "24-bit / 96 kHz" or "320 kbps".
        #[property(get = |o: &Self| o.track().quality_label(), type = String)]
        quality: PhantomData<String>,
        #[property(get = |o: &Self| o.track().format.is_lossless(), type = bool)]
        is_lossless: PhantomData<bool>,
        #[property(get = |o: &Self| o.track().is_hi_res(), type = bool)]
        is_hi_res: PhantomData<bool>,
        /// Whether this is the player's current track. Maintained by the player.
        #[property(get)]
        playing: Cell<bool>,
    }

    impl TrackObject {
        pub(super) fn track(&self) -> &Track {
            self.track
                .get()
                .expect("TrackObject::new always sets the track")
        }

        pub(super) fn set_playing(&self, playing: bool) {
            if self.playing.replace(playing) != playing {
                self.obj().notify_playing();
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TrackObject {
        const NAME: &'static str = "MpTrackObject";
        type Type = super::TrackObject;
    }

    #[glib::derived_properties]
    impl ObjectImpl for TrackObject {}
}

glib::wrapper! {
    /// One library track as a GObject, for list models and property bindings.
    ///
    /// The wrapped [`Track`] is immutable; a changed file gets a new object.
    pub struct TrackObject(ObjectSubclass<imp::TrackObject>);
}

impl TrackObject {
    pub fn new(track: Track) -> Self {
        let object: Self = glib::Object::new();
        object
            .imp()
            .track
            .set(track)
            .expect("a freshly created TrackObject has no track yet");
        object
    }

    /// A track whose search text (see [`Track::search_text`]) was already prepared,
    /// e.g. on a worker thread.
    pub fn with_search_text(track: Track, search_text: String) -> Self {
        let object = Self::new(track);
        object
            .imp()
            .search_text
            .set(search_text)
            .expect("a freshly created TrackObject has no search text yet");
        object
    }

    /// The underlying track data, borrowed without copying.
    pub fn track(&self) -> &Track {
        self.imp().track()
    }

    pub fn track_id(&self) -> TrackId {
        self.track().id
    }

    /// Identifies the album page this track belongs to.
    pub fn album_key(&self) -> AlbumKey {
        AlbumKey::of(self.track())
    }

    /// Identifies the artist page this track is filed under (the normalised album artist).
    pub fn artist_key(&self) -> String {
        mp_core::search::normalize(self.track().display_album_artist())
    }

    /// Normalised text that search queries match against, computed on first use
    /// unless it was prepared with the track.
    pub fn search_text(&self) -> &str {
        self.imp()
            .search_text
            .get_or_init(|| self.track().search_text())
    }

    pub fn matches(&self, query: &SearchQuery) -> bool {
        query.is_empty() || query.matches(self.search_text())
    }

    /// Marks this track as the one playing, notifying only on change.
    pub(crate) fn mark_playing(&self, playing: bool) {
        self.imp().set_playing(playing);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::PathBuf;

    use mp_core::AudioFormat;

    use super::*;

    pub fn track(id: i64, title: &str, artist: &str, album: &str) -> Track {
        Track {
            id: TrackId(id),
            path: PathBuf::from(format!("/music/{artist}/{album}/{title}.flac")),
            title: title.to_owned(),
            artist: artist.to_owned(),
            album: album.to_owned(),
            album_artist: String::new(),
            genre: String::new(),
            year: None,
            track_number: None,
            disc_number: None,
            duration_ms: 200_000,
            format: AudioFormat::Flac,
            sample_rate: Some(96_000),
            bit_depth: Some(24),
            channels: Some(2),
            bitrate_kbps: None,
            file_size: 1,
            modified: 0,
            added: 0,
            cover: None,
        }
    }

    #[test]
    fn exposes_display_properties() {
        let object = TrackObject::new(track(7, "Song", "", "Record"));
        assert_eq!(object.id(), 7);
        assert_eq!(object.artist(), mp_core::model::UNKNOWN_ARTIST);
        assert_eq!(object.album(), "Record");
        assert_eq!(object.disc_number(), 1);
        assert_eq!(object.format(), "FLAC");
        assert!(object.is_hi_res());
        assert!(object.matches(&SearchQuery::new("song record")));
        assert!(!object.matches(&SearchQuery::new("other")));
    }

    #[test]
    fn playing_notifies_only_on_change() {
        let object = TrackObject::new(track(1, "a", "b", "c"));
        let count = std::rc::Rc::new(std::cell::Cell::new(0));
        let counter = count.clone();
        object.connect_playing_notify(move |_| counter.set(counter.get() + 1));
        object.mark_playing(true);
        object.mark_playing(true);
        object.mark_playing(false);
        assert_eq!(count.get(), 2);
    }
}
