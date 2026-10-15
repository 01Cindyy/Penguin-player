//! Albums and artists derived from the flat track list.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::format::AudioFormat;
use crate::model::{CoverId, Track, TrackId, UNKNOWN_ALBUM};
use crate::search::normalize;

/// Identifies an album by normalised album artist and title, so differences in
/// capitalisation or accents between tracks don't split one album into two.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AlbumKey {
    pub artist: String,
    pub title: String,
}

impl AlbumKey {
    pub fn of(track: &Track) -> Self {
        Self {
            artist: normalize(track.display_album_artist()),
            title: normalize(&track.album),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Album {
    pub key: AlbumKey,
    pub title: String,
    pub artist: String,
    pub year: Option<u32>,
    pub cover: Option<CoverId>,
    /// Tracks in album order (disc, track number, title).
    pub tracks: Vec<TrackId>,
    pub duration_ms: u64,
    /// Distinct formats on this album, sorted.
    pub formats: Vec<AudioFormat>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Artist {
    /// Normalised name, used for identity and sorting.
    pub key: String,
    pub name: String,
    /// Albums by this artist, oldest first.
    pub albums: Vec<AlbumKey>,
    pub track_count: usize,
    pub cover: Option<CoverId>,
}

/// The library's default order: album artist, album, disc, track number, then title,
/// ignoring case and accents. Tracks without a disc number count as disc 1, and tracks
/// without a track number come after the numbered ones.
pub fn library_order(track: &Track) -> (AlbumKey, u32, u32, String) {
    let (disc, number, title) = album_order(track);
    (AlbumKey::of(track), disc, number, title)
}

/// The order an album lists its tracks in: disc, then track number, then title.
fn album_order(track: &Track) -> (u32, u32, String) {
    (
        track.disc_number.unwrap_or(1),
        track.track_number.unwrap_or(u32::MAX),
        normalize(&track.title),
    )
}

/// Groups tracks into albums, sorted by album artist then title.
pub fn group_albums(tracks: &[Track]) -> Vec<Album> {
    let mut groups: HashMap<AlbumKey, Vec<&Track>> = HashMap::new();
    for track in tracks {
        groups.entry(AlbumKey::of(track)).or_default().push(track);
    }

    let mut albums: Vec<Album> = groups
        .into_iter()
        .map(|(key, mut members)| {
            members.sort_by_cached_key(|track| album_order(track));
            let first = members[0];
            let mut formats: Vec<AudioFormat> = members.iter().map(|t| t.format).collect();
            formats.sort();
            formats.dedup();
            Album {
                title: if first.album.is_empty() {
                    UNKNOWN_ALBUM.to_owned()
                } else {
                    first.album.clone()
                },
                artist: first.display_album_artist().to_owned(),
                year: members.iter().find_map(|t| t.year),
                cover: members.iter().find_map(|t| t.cover.clone()),
                tracks: members.iter().map(|t| t.id).collect(),
                duration_ms: members.iter().map(|t| t.duration_ms).sum(),
                formats,
                key,
            }
        })
        .collect();
    albums.sort_by(|a, b| a.key.cmp(&b.key));
    albums
}

/// Groups albums (see [`group_albums`]) by album artist, sorted by name.
pub fn group_artists(albums: &[Album]) -> Vec<Artist> {
    let mut albums: Vec<&Album> = albums.iter().collect();
    albums.sort_by(|a, b| {
        let year = |album: &Album| album.year.unwrap_or(u32::MAX);
        year(a).cmp(&year(b)).then_with(|| a.key.cmp(&b.key))
    });

    let mut artists: HashMap<String, Artist> = HashMap::new();
    for album in albums {
        let artist = artists
            .entry(album.key.artist.clone())
            .or_insert_with(|| Artist {
                key: album.key.artist.clone(),
                name: album.artist.clone(),
                albums: Vec::new(),
                track_count: 0,
                cover: None,
            });
        artist.track_count += album.tracks.len();
        artist.albums.push(album.key.clone());
        if artist.cover.is_none() {
            artist.cover = album.cover.clone();
        }
    }

    let mut artists: Vec<Artist> = artists.into_values().collect();
    artists.sort_by(|a, b| a.key.cmp(&b.key));
    artists
}

/// How many tracks use each format, most common first.
pub fn format_counts(tracks: &[Track]) -> Vec<(AudioFormat, usize)> {
    let mut counts: HashMap<AudioFormat, usize> = HashMap::new();
    for track in tracks {
        *counts.entry(track.format).or_default() += 1;
    }
    let mut counts: Vec<_> = counts.into_iter().collect();
    counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    counts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::tests::track;

    fn library() -> Vec<Track> {
        let mut tracks = vec![
            track(1, "Second", "Band", "Album A"),
            track(2, "First", "band", "album a"),
            track(3, "Solo", "Singer", "Album B"),
            track(4, "Loose", "Singer", ""),
        ];
        tracks[0].track_number = Some(2);
        tracks[1].track_number = Some(1);
        tracks[2].year = Some(2001);
        tracks[2].cover = Some(CoverId("b.jpg".into()));
        tracks[3].format = AudioFormat::Mp3;
        tracks
    }

    #[test]
    fn albums_merge_case_variants_and_order_tracks() {
        let albums = group_albums(&library());
        assert_eq!(albums.len(), 3);
        let a = albums.iter().find(|a| a.key.title == "album a").unwrap();
        assert_eq!(a.tracks, vec![TrackId(2), TrackId(1)]);
        assert_eq!(a.duration_ms, 360_000);
        let unknown = albums.iter().find(|a| a.key.title.is_empty()).unwrap();
        assert_eq!(unknown.title, UNKNOWN_ALBUM);
    }

    #[test]
    fn artists_collect_albums() {
        let artists = group_artists(&group_albums(&library()));
        assert_eq!(artists.len(), 2);
        let singer = artists.iter().find(|a| a.key == "singer").unwrap();
        assert_eq!(singer.track_count, 2);
        assert_eq!(singer.albums[0].title, "album b");
        assert_eq!(singer.cover, Some(CoverId("b.jpg".into())));
    }

    #[test]
    fn counts_formats() {
        assert_eq!(
            format_counts(&library()),
            vec![(AudioFormat::Flac, 3), (AudioFormat::Mp3, 1)]
        );
    }
}
