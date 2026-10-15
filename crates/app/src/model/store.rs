use std::cell::RefCell;
use std::collections::HashMap;

use gtk::gio;
use gtk::glib;
use gtk::glib::subclass::prelude::*;
use gtk::prelude::*;
use mp_core::{
    Album, AlbumKey, Artist, AudioFormat, Playlist, PlaylistId, Track, TrackId, collection,
};

use super::album::album_search_text;
use super::{AlbumObject, ArtistObject, PlaylistObject, TrackObject, sync_list_store};

/// Library data prepared off the main thread: tracks in display order plus the albums
/// and artists derived from them. Building one is the expensive part of a refresh,
/// so workers build it and the main thread only swaps it in.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LibrarySnapshot {
    pub tracks: Vec<Track>,
    pub albums: Vec<Album>,
    pub artists: Vec<Artist>,
    /// How many tracks use each format, most common first.
    pub formats: Vec<(AudioFormat, usize)>,
    /// The search text of each track and album, in the order of `tracks` and
    /// `albums`. Normalising it is most of the work of a first search, so it is
    /// prepared here rather than when someone types.
    track_search_texts: Vec<String>,
    album_search_texts: Vec<String>,
}

impl LibrarySnapshot {
    /// Builds the snapshot from tracks in library order (see
    /// [`collection::library_order`]), as `mp_library::Library::tracks` gives them.
    pub fn new(tracks: Vec<Track>) -> Self {
        let albums = collection::group_albums(&tracks);
        let artists = collection::group_artists(&albums);
        let formats = collection::format_counts(&tracks);
        let track_search_texts = tracks.iter().map(Track::search_text).collect();
        let album_search_texts = albums.iter().map(album_search_text).collect();
        Self {
            tracks,
            albums,
            artists,
            formats,
            track_search_texts,
            album_search_texts,
        }
    }

    /// Like [`Self::new`], for tracks in any order.
    #[cfg(test)]
    pub fn from_unsorted(mut tracks: Vec<Track>) -> Self {
        tracks.sort_by_cached_key(collection::library_order);
        Self::new(tracks)
    }
}

mod imp {
    use std::cell::Cell;
    use std::sync::OnceLock;

    use glib::subclass::Signal;

    use super::*;

    #[derive(glib::Properties)]
    #[properties(wrapper_type = super::LibraryStore)]
    pub struct LibraryStore {
        pub(super) tracks: gio::ListStore,
        pub(super) albums: gio::ListStore,
        pub(super) artists: gio::ListStore,
        pub(super) playlists: gio::ListStore,
        pub(super) track_index: RefCell<HashMap<TrackId, TrackObject>>,
        pub(super) album_index: RefCell<HashMap<AlbumKey, AlbumObject>>,
        pub(super) artist_index: RefCell<HashMap<String, ArtistObject>>,
        pub(super) playlist_index: RefCell<HashMap<PlaylistId, PlaylistObject>>,
        pub(super) formats: RefCell<Vec<(AudioFormat, usize)>>,

        /// True once library data has arrived for the first time.
        #[property(get)]
        pub(super) loaded: Cell<bool>,
        #[property(get)]
        pub(super) track_count: Cell<u32>,
        /// Combined length of every track in milliseconds.
        #[property(get)]
        pub(super) total_duration_ms: Cell<u64>,
    }

    impl Default for LibraryStore {
        fn default() -> Self {
            Self {
                tracks: gio::ListStore::new::<TrackObject>(),
                albums: gio::ListStore::new::<AlbumObject>(),
                artists: gio::ListStore::new::<ArtistObject>(),
                playlists: gio::ListStore::new::<PlaylistObject>(),
                track_index: RefCell::default(),
                album_index: RefCell::default(),
                artist_index: RefCell::default(),
                playlist_index: RefCell::default(),
                formats: RefCell::default(),
                loaded: Cell::default(),
                track_count: Cell::default(),
                total_duration_ms: Cell::default(),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for LibraryStore {
        const NAME: &'static str = "MpLibraryStore";
        type Type = super::LibraryStore;
    }

    #[glib::derived_properties]
    impl ObjectImpl for LibraryStore {
        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![
                    Signal::builder("tracks-changed").build(),
                    Signal::builder("playlists-changed").build(),
                ]
            })
        }
    }
}

glib::wrapper! {
    /// The library as GTK list models, shared by every page.
    ///
    /// Tracks, albums, artists and playlists each live in a `gio::ListStore`; views put
    /// filter and sort models on top. Objects whose data didn't change are reused on
    /// refresh, so selections and bindings survive rescans.
    pub struct LibraryStore(ObjectSubclass<imp::LibraryStore>);
}

impl Default for LibraryStore {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl LibraryStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every track as [`TrackObject`]s, in default display order.
    pub fn tracks(&self) -> gio::ListStore {
        self.imp().tracks.clone()
    }

    /// Albums as [`AlbumObject`]s, sorted by album artist then title.
    pub fn albums(&self) -> gio::ListStore {
        self.imp().albums.clone()
    }

    /// Album artists as [`ArtistObject`]s, sorted by name.
    pub fn artists(&self) -> gio::ListStore {
        self.imp().artists.clone()
    }

    /// Playlists as [`PlaylistObject`]s, sorted by name.
    pub fn playlists(&self) -> gio::ListStore {
        self.imp().playlists.clone()
    }

    pub fn track(&self, id: TrackId) -> Option<TrackObject> {
        self.imp().track_index.borrow().get(&id).cloned()
    }

    /// Looks up several tracks, skipping ids that are no longer in the library.
    pub fn tracks_for(&self, ids: &[TrackId]) -> Vec<TrackObject> {
        let index = self.imp().track_index.borrow();
        ids.iter().filter_map(|id| index.get(id).cloned()).collect()
    }

    /// Every track id in default display order.
    pub fn track_ids(&self) -> Vec<TrackId> {
        track_ids_of(&self.imp().tracks)
    }

    pub fn album(&self, key: &AlbumKey) -> Option<AlbumObject> {
        self.imp().album_index.borrow().get(key).cloned()
    }

    /// Looks up an artist by its normalised key (see [`ArtistObject::key`]).
    pub fn artist(&self, key: &str) -> Option<ArtistObject> {
        self.imp().artist_index.borrow().get(key).cloned()
    }

    pub fn playlist(&self, id: PlaylistId) -> Option<PlaylistObject> {
        self.imp().playlist_index.borrow().get(&id).cloned()
    }

    /// How many tracks use each format, most common first.
    pub fn format_counts(&self) -> Vec<(AudioFormat, usize)> {
        self.imp().formats.borrow().clone()
    }

    /// Replaces the library contents, deriving albums and artists on this thread.
    #[cfg(test)]
    pub fn replace_tracks(&self, tracks: Vec<Track>) {
        self.replace(LibrarySnapshot::from_unsorted(tracks));
    }

    /// Swaps in new library data. Tracks keep the snapshot's order.
    pub fn replace(&self, snapshot: LibrarySnapshot) {
        let imp = self.imp();
        let total_duration: u64 = snapshot.tracks.iter().map(|t| t.duration_ms).sum();
        let track_count = snapshot.tracks.len() as u32;

        let tracks = reuse_or_create(
            &imp.track_index,
            snapshot.tracks.into_iter().zip(snapshot.track_search_texts),
            |(track, _)| track.id,
            |object: &TrackObject, (track, _)| object.track() == track,
            |(track, search_text)| TrackObject::with_search_text(track, search_text),
        );
        sync_list_store(&imp.tracks, &tracks);

        let albums = reuse_or_create(
            &imp.album_index,
            snapshot.albums.into_iter().zip(snapshot.album_search_texts),
            |(album, _)| album.key.clone(),
            |object: &AlbumObject, (album, _)| object.album() == album,
            |(album, search_text)| AlbumObject::with_search_text(album, search_text),
        );
        sync_list_store(&imp.albums, &albums);

        let artists = reuse_or_create(
            &imp.artist_index,
            snapshot.artists.into_iter(),
            |artist| artist.key.clone(),
            |object: &ArtistObject, artist| object.artist() == artist,
            ArtistObject::new,
        );
        sync_list_store(&imp.artists, &artists);

        imp.formats.replace(snapshot.formats);
        imp.total_duration_ms.set(total_duration);
        self.notify_total_duration_ms();
        if imp.track_count.replace(track_count) != track_count {
            self.notify_track_count();
        }
        if !imp.loaded.replace(true) {
            self.notify_loaded();
        }
        self.emit_by_name::<()>("tracks-changed", &[]);
    }

    /// Replaces the playlists, updating existing [`PlaylistObject`]s in place.
    pub fn set_playlists(&self, playlists: Vec<Playlist>) {
        let imp = self.imp();
        let mut old = imp.playlist_index.take();
        let mut index = HashMap::with_capacity(playlists.len());
        let objects: Vec<PlaylistObject> = playlists
            .into_iter()
            .map(|playlist| {
                let id = playlist.id;
                let object = match old.remove(&id) {
                    Some(existing) => {
                        existing.update(playlist);
                        existing
                    }
                    None => PlaylistObject::new(playlist),
                };
                index.insert(id, object.clone());
                object
            })
            .collect();
        imp.playlist_index.replace(index);
        sync_list_store(&imp.playlists, &objects);
        self.emit_by_name::<()>("playlists-changed", &[]);
    }

    /// Re-reads playlists from the database, e.g. after editing one.
    pub fn refresh_playlists(&self, library: &mp_library::Library) -> mp_library::Result<()> {
        self.set_playlists(library.playlists()?);
        Ok(())
    }

    /// Called after the track list (and so albums and artists) changed.
    pub fn connect_tracks_changed<F: Fn(&Self) + 'static>(&self, f: F) -> glib::SignalHandlerId {
        self.connect_closure(
            "tracks-changed",
            false,
            glib::closure_local!(move |store: &Self| f(store)),
        )
    }

    /// Called after the playlists were refreshed.
    pub fn connect_playlists_changed<F: Fn(&Self) + 'static>(&self, f: F) -> glib::SignalHandlerId {
        self.connect_closure(
            "playlists-changed",
            false,
            glib::closure_local!(move |store: &Self| f(store)),
        )
    }
}

/// The [`TrackObject`]s in `model`, in model order.
///
/// Use this rather than `ListModel::iter::<TrackObject>()`, which panics on filter and
/// sort models because they report a generic item type.
pub fn tracks_in(model: &impl IsA<gio::ListModel>) -> impl Iterator<Item = TrackObject> + '_ {
    (0..model.n_items()).filter_map(|position| model.item(position).and_downcast())
}

/// Track ids of every [`TrackObject`] in `model`, in model order.
pub fn track_ids_of(model: &impl IsA<gio::ListModel>) -> Vec<TrackId> {
    tracks_in(model).map(|track| track.track_id()).collect()
}

/// Builds the objects for `items`, reusing the old object for any item whose data is
/// unchanged, and rebuilds `index` to match.
fn reuse_or_create<K, T, O>(
    index: &RefCell<HashMap<K, O>>,
    items: impl ExactSizeIterator<Item = T>,
    key_of: impl Fn(&T) -> K,
    unchanged: impl Fn(&O, &T) -> bool,
    create: impl Fn(T) -> O,
) -> Vec<O>
where
    K: std::hash::Hash + Eq,
    O: Clone,
{
    let mut old = index.take();
    let mut new_index = HashMap::with_capacity(items.len());
    let objects = items
        .map(|item| {
            let key = key_of(&item);
            let object = match old.remove(&key) {
                Some(existing) if unchanged(&existing, &item) => existing,
                _ => create(item),
            };
            new_index.insert(key, object.clone());
            object
        })
        .collect();
    index.replace(new_index);
    objects
}

#[cfg(test)]
mod tests {
    use mp_core::{PlaylistEntry, PlaylistItemId, SearchQuery};

    use super::*;
    use crate::model::track::tests::track;

    fn library() -> Vec<Track> {
        let mut tracks = vec![
            track(1, "Second", "Zed", "Beta"),
            track(2, "First", "Zed", "Beta"),
            track(3, "Opening", "Ábba", "Alpha"),
        ];
        tracks[0].track_number = Some(2);
        tracks[1].track_number = Some(1);
        tracks
    }

    #[test]
    fn snapshot_sorts_and_groups() {
        let snapshot = LibrarySnapshot::from_unsorted(library());
        let ids: Vec<i64> = snapshot.tracks.iter().map(|t| t.id.0).collect();
        assert_eq!(ids, vec![3, 2, 1]);
        assert_eq!(snapshot.albums.len(), 2);
        assert_eq!(snapshot.artists.len(), 2);
    }

    #[test]
    fn snapshot_prepares_search_texts_in_order() {
        let snapshot = LibrarySnapshot::from_unsorted(library());
        let expected: Vec<String> = snapshot.tracks.iter().map(Track::search_text).collect();
        assert_eq!(snapshot.track_search_texts, expected);
        let expected: Vec<String> = snapshot.albums.iter().map(album_search_text).collect();
        assert_eq!(snapshot.album_search_texts, expected);

        let store = LibraryStore::new();
        store.replace(snapshot);
        let opening = store.track(TrackId(3)).unwrap();
        assert_eq!(opening.search_text(), opening.track().search_text());
    }

    #[test]
    fn replace_reuses_unchanged_objects() {
        let store = LibraryStore::new();
        store.replace_tracks(library());
        assert!(store.loaded());
        assert_eq!(store.track_count(), 3);
        assert_eq!(store.total_duration_ms(), 600_000);
        assert_eq!(store.track_ids(), vec![TrackId(3), TrackId(2), TrackId(1)]);

        let unchanged = store.track(TrackId(3)).unwrap();
        let before_edit = store.track(TrackId(2)).unwrap();
        let album_before = store.album(&unchanged.album_key()).unwrap();

        let mut tracks = library();
        tracks[1].title = "First (Remastered)".into();
        tracks.pop();
        tracks.push(track(3, "Opening", "Ábba", "Alpha"));
        store.replace_tracks(tracks);

        assert_eq!(store.track(TrackId(3)).unwrap(), unchanged);
        assert_ne!(store.track(TrackId(2)).unwrap(), before_edit);
        assert_eq!(store.album(&unchanged.album_key()).unwrap(), album_before);
        assert_eq!(store.tracks_for(&[TrackId(1), TrackId(99)]).len(), 1);
        assert!(store.artist("zed").is_some());
    }

    /// Views on the store redo only what a refresh changed, which matters with tens
    /// of thousands of tracks.
    #[test]
    fn refreshes_splice_only_the_changed_items() {
        let store = LibraryStore::new();
        store.replace_tracks(library());
        let changes = std::rc::Rc::new(RefCell::new(Vec::new()));
        let log = changes.clone();
        store
            .tracks()
            .connect_items_changed(move |_, position, removed, added| {
                log.borrow_mut().push((position, removed, added));
            });

        store.replace_tracks(library());
        let mut tracks = library();
        tracks[0].genre = "Jazz".into();
        store.replace_tracks(tracks);
        assert_eq!(*changes.borrow(), [(2, 1, 1)]);
    }

    #[test]
    fn albums_and_artists_match_searches() {
        let store = LibraryStore::new();
        store.replace_tracks(library());
        let key = AlbumKey {
            artist: "abba".into(),
            title: "alpha".into(),
        };
        let album = store.album(&key).unwrap();
        assert!(album.matches(&SearchQuery::new("ABBA alp")));
        assert!(!album.matches(&SearchQuery::new("beta")));
        let artist = store.artist("zed").unwrap();
        assert!(artist.matches(&SearchQuery::new("Zed")));
        assert!(!artist.matches(&SearchQuery::new("abba")));
    }

    #[test]
    fn track_ids_of_generic_models() {
        // Filter and sort models report a plain `GObject` item type, like this store.
        let model = gio::ListStore::new::<glib::Object>();
        for id in [3, 1] {
            model.append(&TrackObject::new(track(id, "a", "b", "c")));
        }
        assert_eq!(track_ids_of(&model), vec![TrackId(3), TrackId(1)]);
    }

    #[test]
    fn playlists_update_in_place() {
        let store = LibraryStore::new();
        let playlist = |name: &str, entries: usize| Playlist {
            id: PlaylistId(1),
            name: name.to_owned(),
            created: 0,
            modified: 0,
            entries: (0..entries)
                .map(|i| PlaylistEntry {
                    item: PlaylistItemId(i as i64),
                    path: format!("/music/{i}.flac").into(),
                    track: Some(TrackId(i as i64)),
                })
                .collect(),
        };
        store.set_playlists(vec![playlist("Mix", 1)]);
        let object = store.playlist(PlaylistId(1)).unwrap();

        let changes = std::rc::Rc::new(std::cell::Cell::new(0));
        let counter = changes.clone();
        object.connect_changed(move |_| counter.set(counter.get() + 1));

        store.set_playlists(vec![playlist("Mix", 2)]);
        assert_eq!(store.playlist(PlaylistId(1)).unwrap(), object);
        assert_eq!(object.track_count(), 2);
        assert_eq!(changes.get(), 1);

        store.set_playlists(Vec::new());
        assert_eq!(store.playlists().n_items(), 0);
    }
}
