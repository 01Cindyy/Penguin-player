//! Finding and ranking what matches a search.
//!
//! Matching uses [`SearchQuery`] on each object's cached search text, so a search is a
//! substring scan with no allocation per item. Ranking sorts matches into a few
//! [`Relevance`] buckets, which keeps it linear and keeps library order within a
//! bucket. While the user types more of a word, only the previous matches are
//! searched again (see [`Query::narrows`]).

use gtk::prelude::*;
use mp_core::SearchQuery;
use mp_core::search::normalize;

use crate::model::{AlbumObject, ArtistObject, LibraryStore, TrackObject};
use crate::ui::widgets::discover::TopResult;

/// How well a result matches, best first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Relevance {
    /// The name is the query.
    Exact,
    /// The name starts with the query.
    Prefix,
    /// A word of the name or the details starts with the query's first word.
    WordStart,
    /// Every word of the query appears somewhere.
    Contains,
}

impl Relevance {
    const ALL: [Relevance; 4] = [
        Relevance::Exact,
        Relevance::Prefix,
        Relevance::WordStart,
        Relevance::Contains,
    ];
}

/// A search query, normalised for matching and ranking.
#[derive(Debug, Clone, Default)]
pub struct Query {
    matcher: SearchQuery,
    terms: Vec<String>,
    /// The terms joined by single spaces, to compare with whole names.
    text: String,
}

impl Query {
    pub fn new(query: &str) -> Self {
        let terms: Vec<String> = normalize(query)
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        Self {
            matcher: SearchQuery::new(query),
            text: terms.join(" "),
            terms,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// The normalised query: lowercase, without accents, single spaces.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Whether everything that matches `self` also matched `broader`, as when the user
    /// types more of a word: then only `broader`'s matches need checking.
    pub fn narrows(&self, broader: &Query) -> bool {
        !broader.is_empty()
            && broader
                .terms
                .iter()
                .all(|old| self.terms.iter().any(|new| new.contains(old.as_str())))
    }

    /// How well `name` (normalised) and the rest of `haystack` match. Only call this
    /// for candidates the query matches.
    fn relevance(&self, name: &str, haystack: &str) -> Relevance {
        if name == self.text {
            Relevance::Exact
        } else if name.starts_with(&self.text) {
            Relevance::Prefix
        } else if self.first_term_starts_word(haystack) {
            Relevance::WordStart
        } else {
            Relevance::Contains
        }
    }

    fn first_term_starts_word(&self, text: &str) -> bool {
        let Some(term) = self.terms.first() else {
            return false;
        };
        text.match_indices(term.as_str()).any(|(index, _)| {
            text[..index]
                .chars()
                .next_back()
                .is_none_or(|before| !before.is_alphanumeric())
        })
    }
}

/// Something a search can find.
pub trait Candidate: Clone {
    /// How well `query` matches this, or `None` if it doesn't.
    fn rank(&self, query: &Query) -> Option<Relevance>;
}

impl Candidate for TrackObject {
    fn rank(&self, query: &Query) -> Option<Relevance> {
        if !self.matches(&query.matcher) {
            return None;
        }
        let haystack = self.search_text();
        // The search text starts with the title, so only titles that start with the
        // first term can be exact or prefix matches; only those are normalised.
        let title_may_match = query
            .terms
            .first()
            .is_some_and(|term| haystack.starts_with(term.as_str()));
        Some(if title_may_match {
            query.relevance(&normalize(&self.track().title), haystack)
        } else if query.first_term_starts_word(haystack) {
            Relevance::WordStart
        } else {
            Relevance::Contains
        })
    }
}

impl Candidate for AlbumObject {
    fn rank(&self, query: &Query) -> Option<Relevance> {
        self.matches(&query.matcher)
            .then(|| query.relevance(&self.key().title, self.search_text()))
    }
}

impl Candidate for ArtistObject {
    fn rank(&self, query: &Query) -> Option<Relevance> {
        self.matches(&query.matcher)
            .then(|| query.relevance(self.search_text(), self.search_text()))
    }
}

/// What a search can find, as plain vectors so a search doesn't go through a GObject
/// list model call per item. Rebuilt when the library changes.
#[derive(Default)]
pub struct Searchable {
    tracks: Vec<TrackObject>,
    albums: Vec<AlbumObject>,
    artists: Vec<ArtistObject>,
}

impl Searchable {
    pub fn new(store: &LibraryStore) -> Self {
        Self {
            tracks: store.tracks().iter().filter_map(Result::ok).collect(),
            albums: store.albums().iter().filter_map(Result::ok).collect(),
            artists: store.artists().iter().filter_map(Result::ok).collect(),
        }
    }

    #[cfg(test)]
    fn from_parts(
        tracks: Vec<TrackObject>,
        albums: Vec<AlbumObject>,
        artists: Vec<ArtistObject>,
    ) -> Self {
        Self {
            tracks,
            albums,
            artists,
        }
    }
}

/// The matches of one kind.
#[derive(Debug, Clone)]
pub struct Matches<T> {
    /// Best first; equally good matches keep library order.
    ranked: Vec<T>,
    /// In library order, to search again when the query narrows.
    in_order: Vec<T>,
    best: Option<Relevance>,
}

impl<T> Default for Matches<T> {
    fn default() -> Self {
        Self {
            ranked: Vec::new(),
            in_order: Vec::new(),
            best: None,
        }
    }
}

impl<T: Candidate> Matches<T> {
    fn find(candidates: &[T], query: &Query) -> Self {
        let mut buckets: [Vec<T>; 4] = Default::default();
        let mut in_order = Vec::new();
        for candidate in candidates {
            if let Some(relevance) = candidate.rank(query) {
                buckets[relevance as usize].push(candidate.clone());
                in_order.push(candidate.clone());
            }
        }
        let best = Relevance::ALL
            .into_iter()
            .find(|&relevance| !buckets[relevance as usize].is_empty());
        Self {
            ranked: buckets.into_iter().flatten().collect(),
            in_order,
            best,
        }
    }

    /// The best match and how good it is.
    fn top(&self) -> Option<(Relevance, &T)> {
        self.best.zip(self.ranked.first())
    }
}

impl<T> std::ops::Deref for Matches<T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        &self.ranked
    }
}

/// Everything that matches a query.
#[derive(Debug, Clone, Default)]
pub struct SearchResults {
    pub query: Query,
    pub songs: Matches<TrackObject>,
    pub albums: Matches<AlbumObject>,
    pub artists: Matches<ArtistObject>,
    /// The single best match, for the top result card and Enter.
    pub top: Option<TopResult>,
}

impl SearchResults {
    /// Searches `library` for `query`. If `previous` holds results for a query this one
    /// narrows (from the same library), only those results are searched.
    pub fn find(library: &Searchable, query: Query, previous: Option<&SearchResults>) -> Self {
        if query.is_empty() {
            return Self {
                query,
                ..Self::default()
            };
        }
        let narrowed = previous.filter(|previous| query.narrows(&previous.query));
        let (tracks, albums, artists) = match narrowed {
            Some(previous) => (
                previous.songs.in_order.as_slice(),
                previous.albums.in_order.as_slice(),
                previous.artists.in_order.as_slice(),
            ),
            None => (
                library.tracks.as_slice(),
                library.albums.as_slice(),
                library.artists.as_slice(),
            ),
        };
        let songs = Matches::find(tracks, &query);
        let albums = Matches::find(albums, &query);
        let artists = Matches::find(artists, &query);
        let top = top_result(&songs, &albums, &artists);
        Self {
            query,
            songs,
            albums,
            artists,
            top,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.songs.is_empty() && self.albums.is_empty() && self.artists.is_empty()
    }
}

/// The best match overall. When an artist, an album and a song match equally well,
/// the artist wins, then the album: they lead to more of what was searched for.
fn top_result(
    songs: &Matches<TrackObject>,
    albums: &Matches<AlbumObject>,
    artists: &Matches<ArtistObject>,
) -> Option<TopResult> {
    let candidates = [
        artists
            .top()
            .map(|(relevance, artist)| (relevance, TopResult::Artist(artist.clone()))),
        albums
            .top()
            .map(|(relevance, album)| (relevance, TopResult::Album(album.clone()))),
        songs
            .top()
            .map(|(relevance, song)| (relevance, TopResult::Song(song.clone()))),
    ];
    // `min_by_key` keeps the first of equal elements, so the order above breaks ties.
    candidates
        .into_iter()
        .flatten()
        .min_by_key(|(relevance, _)| *relevance)
        .map(|(_, result)| result)
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use mp_core::Track;

    use super::*;
    use crate::model::{LibrarySnapshot, test_track};

    fn library(tracks: Vec<Track>) -> Searchable {
        let snapshot = LibrarySnapshot::from_unsorted(tracks);
        Searchable::from_parts(
            snapshot.tracks.into_iter().map(TrackObject::new).collect(),
            snapshot.albums.into_iter().map(AlbumObject::new).collect(),
            snapshot
                .artists
                .into_iter()
                .map(ArtistObject::new)
                .collect(),
        )
    }

    fn titles(results: &SearchResults) -> Vec<String> {
        results.songs.iter().map(TrackObject::title).collect()
    }

    fn search(library: &Searchable, query: &str) -> SearchResults {
        SearchResults::find(library, Query::new(query), None)
    }

    #[test]
    fn ranks_names_before_other_matches() {
        let library = library(vec![
            test_track(1, "Midnight", "Aurora", "Skies"),
            test_track(2, "Night Drive", "Aurora", "Skies"),
            test_track(3, "Late Night", "Aurora", "Skies"),
            test_track(4, "Night", "Aurora", "Skies"),
            test_track(5, "Calm", "Aurora", "Night Moves"),
        ]);
        let results = search(&library, "night");
        assert_eq!(
            titles(&results),
            ["Night", "Night Drive", "Calm", "Late Night", "Midnight"]
        );
        assert!(matches!(results.top, Some(TopResult::Song(ref song)) if song.title() == "Night"));
        assert_eq!(results.albums.len(), 1);
        assert!(results.artists.is_empty());
    }

    #[test]
    fn prefers_artists_and_albums_on_ties() {
        let catalogue = library(vec![
            test_track(1, "Björk", "Björk", "Debut"),
            test_track(2, "Debut", "Someone Else", "Singles"),
        ]);
        let artist = search(&catalogue, "bjork");
        assert!(matches!(artist.top, Some(TopResult::Artist(ref a)) if a.name() == "Björk"));
        let album = search(&catalogue, "debut");
        assert!(matches!(album.top, Some(TopResult::Album(ref a)) if a.title() == "Debut"));
        // An exact song title beats an album whose title only starts with the query.
        let catalogue = library(vec![
            test_track(1, "Blue", "Ann", "Blue Hour"),
            test_track(2, "Other", "Ann", "Blue Hour"),
        ]);
        let song = search(&catalogue, "blue");
        assert!(matches!(song.top, Some(TopResult::Song(ref s)) if s.title() == "Blue"));
    }

    #[test]
    fn narrowing_searches_previous_matches_only() {
        assert!(Query::new("nig dr").narrows(&Query::new("nig")));
        assert!(Query::new("night").narrows(&Query::new("nig")));
        assert!(!Query::new("nig").narrows(&Query::new("night")));
        assert!(!Query::new("drive").narrows(&Query::new("nig")));
        assert!(!Query::new("x").narrows(&Query::new("")));

        let library = library(vec![
            test_track(1, "Night Drive", "A", "B"),
            test_track(2, "Nightfall", "A", "B"),
            test_track(3, "Day", "A", "B"),
        ]);
        let broad = search(&library, "nig");
        let narrow = SearchResults::find(&library, Query::new("night d"), Some(&broad));
        assert_eq!(titles(&narrow), ["Night Drive"]);
        // A query that doesn't narrow searches the whole library again.
        let other = SearchResults::find(&library, Query::new("day"), Some(&narrow));
        assert_eq!(titles(&other), ["Day"]);
    }

    #[test]
    fn empty_and_unmatched_queries() {
        let library = library(vec![test_track(1, "Song", "A", "B")]);
        assert!(search(&library, "  ").is_empty());
        assert!(search(&library, "´").is_empty());
        let none = search(&library, "zzz");
        assert!(none.is_empty());
        assert!(none.top.is_none());
    }

    /// Rough timing of a first search in a large library. Not run by default:
    /// `cargo test -p penguin-player --release -- --ignored --nocapture first_search`
    #[test]
    #[ignore = "benchmark: run explicitly with --ignored"]
    fn first_search_timing() {
        let tracks: Vec<Track> = (0..50_000)
            .map(|id| {
                let artist = format!("Artist {}", id % 900);
                test_track(
                    id,
                    &format!("Song {id}"),
                    &artist,
                    &format!("Album {}", id / 10),
                )
            })
            .collect();
        let started = Instant::now();
        let snapshot = LibrarySnapshot::from_unsorted(tracks);
        println!("snapshot (on a worker): {:?}", started.elapsed());
        let store = LibraryStore::new();
        let started = Instant::now();
        store.replace(snapshot);
        println!("replace: {:?}", started.elapsed());

        let library = Searchable::new(&store);
        let started = Instant::now();
        let results = search(&library, "song 4");
        println!(
            "first search: {} songs in {:?}",
            results.songs.len(),
            started.elapsed()
        );
    }

    #[test]
    fn word_starts_ignore_punctuation() {
        let query = Query::new("live");
        assert!(query.first_term_starts_word("song (live) artist"));
        assert!(query.first_term_starts_word("live at home"));
        assert!(!query.first_term_starts_word("alive"));
    }
}
