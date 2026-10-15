//! What the library holds by one artist: their albums, their songs and the albums by
//! others they appear on.

use std::collections::HashSet;

use gtk::prelude::*;
use mp_core::search::normalize;
use mp_core::{AlbumKey, Track};

use crate::model::{AlbumObject, ArtistObject, LibraryStore, TrackObject};

/// An artist's music, in the order the artist page shows it.
#[derive(Debug, Default)]
pub struct Discography {
    /// Albums filed under the artist, oldest first.
    pub albums: Vec<AlbumObject>,
    /// Albums filed under someone else with tracks by the artist (compilations,
    /// guest appearances), oldest first.
    pub appearances: Vec<AlbumObject>,
    /// The artist's albums in order, then their tracks on [`Self::appearances`].
    pub tracks: Vec<TrackObject>,
}

impl Discography {
    /// Collects `artist`'s music from `store`. Finding appearances looks at every
    /// track once, so build this once per page or library change, not per item.
    pub fn of(store: &LibraryStore, artist: &ArtistObject) -> Self {
        let albums: Vec<AlbumObject> = artist
            .album_keys()
            .iter()
            .filter_map(|key| store.album(key))
            .collect();
        let mut tracks: Vec<TrackObject> = albums
            .iter()
            .flat_map(|album| store.tracks_for(album.track_ids()))
            .collect();

        let mut appearances = appearance_albums(store, artist.key());
        // Unknown years (0) go last.
        appearances.sort_by_key(|album| match album.year() {
            0 => u32::MAX,
            year => year,
        });
        for album in &appearances {
            tracks.extend(
                store
                    .tracks_for(album.track_ids())
                    .into_iter()
                    .filter(|track| is_guest_track(track.track(), artist.key())),
            );
        }
        Self {
            albums,
            appearances,
            tracks,
        }
    }
}

/// Albums with a track by `artist_key` that are filed under another artist, in
/// library order.
fn appearance_albums(store: &LibraryStore, artist_key: &str) -> Vec<AlbumObject> {
    let tracks = store.tracks();
    let mut seen: HashSet<AlbumKey> = HashSet::new();
    (0..tracks.n_items())
        .filter_map(|position| tracks.item(position).and_downcast::<TrackObject>())
        .filter(|track| is_guest_track(track.track(), artist_key))
        .filter_map(|track| {
            let key = track.album_key();
            seen.insert(key.clone())
                .then(|| store.album(&key))
                .flatten()
        })
        .collect()
}

/// Whether `track` is by the artist with `artist_key` but filed under someone else.
///
/// Only tracks tagged with a different album artist can be guest tracks, so the
/// costly normalisation runs for few tracks, typically those on compilations.
fn is_guest_track(track: &Track, artist_key: &str) -> bool {
    !track.album_artist.is_empty()
        && track.album_artist != track.artist
        && normalize(track.display_artist()) == artist_key
        && normalize(&track.album_artist) != artist_key
}

#[cfg(test)]
mod tests {
    use mp_core::TrackId;

    use super::*;
    use crate::model::test_track;

    fn library() -> Vec<Track> {
        let mut own_old = test_track(1, "Early", "Mira", "First Light");
        own_old.year = Some(2001);
        let mut own_new = test_track(2, "Late", "Mira", "Second Wind");
        own_new.year = Some(2010);
        let mut guest = test_track(3, "Duet", "Míra", "Summer Hits");
        guest.album_artist = "Various Artists".into();
        guest.year = Some(2005);
        let mut other = test_track(4, "Filler", "Someone", "Summer Hits");
        other.album_artist = "Various Artists".into();
        let unrelated = test_track(5, "Elsewhere", "Nobody", "Quiet");
        vec![own_new, guest, own_old, other, unrelated]
    }

    #[test]
    fn collects_albums_appearances_and_tracks() {
        let store = LibraryStore::new();
        store.replace_tracks(library());
        let artist = store.artist("mira").expect("Mira is an album artist");

        let discography = Discography::of(&store, &artist);
        let titles: Vec<String> = discography.albums.iter().map(|a| a.title()).collect();
        assert_eq!(titles, ["First Light", "Second Wind"]);
        let guest_on: Vec<String> = discography.appearances.iter().map(|a| a.title()).collect();
        assert_eq!(guest_on, ["Summer Hits"]);
        let ids: Vec<TrackId> = discography.tracks.iter().map(|t| t.track_id()).collect();
        assert_eq!(ids, [TrackId(1), TrackId(2), TrackId(3)]);
    }

    #[test]
    fn album_artists_are_not_their_own_guests() {
        let mut track = test_track(1, "Song", "Band", "Record");
        assert!(!is_guest_track(&track, "band"));
        track.album_artist = "BAND".into();
        assert!(!is_guest_track(&track, "band"));
        track.album_artist = "Various Artists".into();
        assert!(is_guest_track(&track, "band"));
        assert!(!is_guest_track(&track, "various artists"));
    }
}
