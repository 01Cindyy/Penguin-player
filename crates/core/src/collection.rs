//! Albums and artists derived from the flat track list.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::format::AudioFormat;
use crate::model::{CoverId, Track, TrackId, UNKNOWN_ALBUM};
use crate::search::normalize;

/// The album artist of albums by several artists whose tags name none.
pub const VARIOUS_ARTISTS: &str = "Various Artists";

/// Gives tracks without an album artist tag the one their album is filed under, where
/// the folder they are in says more than the track alone: the tracks of one album are
/// those in the same folder with the same album title. Without this, an album by
/// several artists (a compilation, a soundtrack, or one with guests on some tracks)
/// would be split into one album per artist.
///
/// - If the album's other tracks agree on a tagged album artist, it is theirs too.
/// - If none is tagged and the artists differ, it is the artist every other one starts
///   with ("Mira" for "Mira" and "Mira feat. Sol"), or else [`VARIOUS_ARTISTS`].
///
/// Tracks of an album by one artist are left as they are, as are tracks without an
/// album title.
pub fn infer_album_artists(tracks: &mut [Track]) {
    let mut albums: HashMap<(&Path, String), Vec<usize>> = HashMap::new();
    for (index, track) in tracks.iter().enumerate() {
        if let Some(folder) = track.path.parent().filter(|_| !track.album.is_empty()) {
            let album = (folder, normalize(&track.album));
            albums.entry(album).or_default().push(index);
        }
    }
    let mut inferred = Vec::new();
    for members in albums.into_values().filter(|members| members.len() > 1) {
        let album = || members.iter().map(|&index| &tracks[index]);
        if album().all(|track| !track.album_artist.is_empty()) {
            continue;
        }
        if let Some(artist) = shared_album_artist(album()) {
            let untagged = members
                .iter()
                .filter(|&&index| tracks[index].album_artist.is_empty());
            inferred.extend(untagged.map(|&index| (index, artist.clone())));
        }
    }
    for (index, artist) in inferred {
        tracks[index].album_artist = artist;
    }
}

/// The album artist for the untagged tracks among `album`'s (see
/// [`infer_album_artists`]), or `None` to leave them as they are.
fn shared_album_artist<'a>(album: impl Iterator<Item = &'a Track>) -> Option<String> {
    // By normalised name, so the choice doesn't depend on the order of the tracks.
    let mut tagged: BTreeMap<String, &str> = BTreeMap::new();
    let mut artists: BTreeMap<String, &str> = BTreeMap::new();
    for track in album {
        if track.album_artist.is_empty() {
            artists
                .entry(normalize(&track.artist))
                .or_insert(&track.artist);
        } else {
            let name = track.album_artist.as_str();
            tagged.entry(normalize(name)).or_insert(name);
        }
    }
    match tagged.len() {
        0 if artists.len() < 2 => None,
        0 => {
            let (lead, name) = artists.iter().min_by_key(|(key, _)| key.len())?;
            let led = !lead.is_empty()
                && artists.keys().all(|artist| {
                    artist
                        .strip_prefix(lead.as_str())
                        .is_some_and(|rest| !rest.starts_with(|next: char| next.is_alphanumeric()))
                });
            Some(if led {
                (*name).to_owned()
            } else {
                VARIOUS_ARTISTS.to_owned()
            })
        }
        1 => tagged.into_values().next().map(str::to_owned),
        // Two albums with one title in one folder: nothing says which a track is on.
        _ => None,
    }
}

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

    /// Tracks in `folder`, as (title, artist, album, album artist).
    fn folder(folder: &str, songs: &[(&str, &str, &str, &str)]) -> Vec<Track> {
        songs
            .iter()
            .enumerate()
            .map(|(index, (title, artist, album, album_artist))| {
                let mut song = track(index as i64, title, artist, album);
                song.path = format!("/music/{folder}/{title}.flac").into();
                song.album_artist = (*album_artist).to_owned();
                song
            })
            .collect()
    }

    fn album_artists(tracks: &[Track]) -> Vec<&str> {
        tracks
            .iter()
            .map(|track| track.album_artist.as_str())
            .collect()
    }

    #[test]
    fn albums_by_several_artists_stay_together() {
        let mut tracks = folder(
            "Summer Mix",
            &[
                ("One", "Mira", "Summer Mix", ""),
                ("Two", "Sol", "Summer Mix", ""),
                ("Three", "", "Summer Mix", ""),
                ("Loose", "Someone", "", ""),
                ("Other", "Someone Else", "", ""),
            ],
        );
        infer_album_artists(&mut tracks);
        assert_eq!(
            album_artists(&tracks),
            [VARIOUS_ARTISTS, VARIOUS_ARTISTS, VARIOUS_ARTISTS, "", ""]
        );
        let albums = group_albums(&tracks);
        let mix = albums.iter().find(|a| a.title == "Summer Mix").unwrap();
        assert_eq!(
            (mix.artist.as_str(), mix.tracks.len()),
            (VARIOUS_ARTISTS, 3)
        );
    }

    #[test]
    fn albums_with_guests_are_filed_under_their_artist() {
        let mut tracks = folder(
            "Tides",
            &[
                ("One", "Mira feat. Sol", "Tides", ""),
                ("Two", "mira", "TIDES", ""),
                ("Three", "Mira & The Band", "Tides", ""),
            ],
        );
        infer_album_artists(&mut tracks);
        assert_eq!(album_artists(&tracks), ["mira", "mira", "mira"]);
        assert_eq!(group_albums(&tracks).len(), 1);

        // "Mirage" only starts with the same letters.
        let mut tracks = folder(
            "Mixed",
            &[("One", "Mira", "Mixed", ""), ("Two", "Mirage", "Mixed", "")],
        );
        infer_album_artists(&mut tracks);
        assert_eq!(album_artists(&tracks), [VARIOUS_ARTISTS, VARIOUS_ARTISTS]);
    }

    #[test]
    fn untagged_tracks_take_the_album_artist_of_their_album() {
        let mut tracks = folder(
            "Tides",
            &[
                ("One", "Mira", "Tides", "Mira"),
                ("Two", "Mira feat. Sol", "Tides", ""),
                ("Three", "Sol", "Tides", ""),
            ],
        );
        infer_album_artists(&mut tracks);
        assert_eq!(album_artists(&tracks), ["Mira", "Mira", "Mira"]);

        // Two albums of one title in one folder: nothing says which the third is on.
        let mut tracks = folder(
            "Hits",
            &[
                ("One", "Mira", "Hits", "Mira"),
                ("Two", "Sol", "Hits", "Sol"),
                ("Three", "Ana", "Hits", ""),
            ],
        );
        infer_album_artists(&mut tracks);
        assert_eq!(album_artists(&tracks), ["Mira", "Sol", ""]);
    }

    #[test]
    fn albums_by_one_artist_and_albums_in_other_folders_are_left_alone() {
        let mut tracks = folder(
            "Mira/Hits",
            &[("One", "Mira", "Hits", ""), ("Two", "Mira", "Hits", "")],
        );
        tracks.extend(folder("Sol/Hits", &[("Three", "Sol", "Hits", "")]));
        let before = tracks.clone();
        infer_album_artists(&mut tracks);
        assert_eq!(tracks, before);
        assert_eq!(group_albums(&tracks).len(), 2);
    }

    #[test]
    fn counts_formats() {
        assert_eq!(
            format_counts(&library()),
            vec![(AudioFormat::Flac, 3), (AudioFormat::Mp3, 1)]
        );
    }
}
