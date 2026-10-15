//! Playlists and the queue can hold a song more than once, but GTK list views need
//! every item to be a different object. [`DistinctRows`] stands in copies for the
//! repeats.

use std::collections::HashMap;

use mp_core::TrackId;

use crate::model::TrackObject;

/// Hands out copies of [`TrackObject`]s for tracks that repeat within a list.
///
/// The first appearance of a track keeps the library's object (so it shows when it
/// is playing); the n-th repeat always gets the same copy, so rows that didn't change
/// keep their objects from one update to the next.
#[derive(Debug, Default)]
pub struct DistinctRows {
    /// For each repeated track: the library object the copies were made from, and
    /// the copies for its first, second, … repeat.
    copies: HashMap<TrackId, (TrackObject, Vec<TrackObject>)>,
}

impl DistinctRows {
    /// `tracks` with every repeat of a track replaced by its copy.
    pub fn apply(&mut self, tracks: Vec<TrackObject>) -> Vec<TrackObject> {
        let mut seen: HashMap<TrackId, usize> = HashMap::with_capacity(tracks.len());
        let rows = tracks
            .into_iter()
            .map(|track| {
                let count = seen.entry(track.track_id()).or_default();
                *count += 1;
                match *count {
                    1 => track,
                    appearance => self.copy(&track, appearance - 2),
                }
            })
            .collect();
        self.forget_unused(&seen);
        rows
    }

    fn copy(&mut self, track: &TrackObject, repeat: usize) -> TrackObject {
        let (source, copies) = self
            .copies
            .entry(track.track_id())
            .or_insert_with(|| (track.clone(), Vec::new()));
        // A rescanned file has a new library object; its old copies are outdated.
        if source != track {
            *source = track.clone();
            copies.clear();
        }
        while copies.len() <= repeat {
            copies.push(TrackObject::new(track.track().clone()));
        }
        copies[repeat].clone()
    }

    /// Drops copies for repeats the list no longer has.
    fn forget_unused(&mut self, seen: &HashMap<TrackId, usize>) {
        self.copies.retain(|id, (_, copies)| {
            let repeats = seen.get(id).map_or(0, |count| count - 1);
            copies.truncate(repeats);
            repeats > 0
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::test_track;

    #[test]
    fn repeats_get_stable_copies() {
        let a = TrackObject::new(test_track(1, "a", "x", "y"));
        let b = TrackObject::new(test_track(2, "b", "x", "y"));
        let mut distinct = DistinctRows::default();

        let rows = distinct.apply(vec![a.clone(), b.clone(), a.clone(), a.clone()]);
        assert_eq!(rows[0], a);
        assert_eq!(rows[1], b);
        assert_ne!(rows[2], a);
        assert_ne!(rows[2], rows[3]);
        assert_eq!(rows[2].track_id(), a.track_id());

        // The same repeats get the same copies; the library object stays first.
        let again = distinct.apply(vec![b.clone(), a.clone(), a.clone(), a.clone()]);
        assert_eq!(again[1], a);
        assert_eq!(again[2], rows[2]);
        assert_eq!(again[3], rows[3]);

        // Fewer repeats drop the extra copies; a rescanned track gets new ones.
        distinct.apply(vec![a.clone(), a.clone()]);
        assert_eq!(distinct.copies[&a.track_id()].1.len(), 1);
        let rescanned = TrackObject::new(test_track(1, "a (remastered)", "x", "y"));
        let fresh = distinct.apply(vec![rescanned.clone(), rescanned.clone()]);
        assert_ne!(fresh[1], rows[2]);
        assert_eq!(fresh[1].title(), "a (remastered)");
        distinct.apply(vec![b]);
        assert!(distinct.copies.is_empty());
    }
}
