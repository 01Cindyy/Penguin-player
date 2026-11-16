use serde::{Deserialize, Serialize};

use crate::model::TrackId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RepeatMode {
    #[default]
    Off,
    All,
    One,
}

impl RepeatMode {
    /// Off → All → One → Off, the order a repeat button steps through.
    pub fn cycle(self) -> Self {
        match self {
            RepeatMode::Off => RepeatMode::All,
            RepeatMode::All => RepeatMode::One,
            RepeatMode::One => RepeatMode::Off,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            RepeatMode::Off => "off",
            RepeatMode::All => "all",
            RepeatMode::One => "one",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "off" => Some(RepeatMode::Off),
            "all" => Some(RepeatMode::All),
            "one" => Some(RepeatMode::One),
            _ => None,
        }
    }
}

/// The tracks being played and where playback is within them.
///
/// `items` holds tracks in the order they were added. `order` is the play order as
/// indices into `items`: the identity while shuffle is off, a permutation while it is on.
/// `cursor` is a position in `order` and is `Some` exactly when the queue is not empty.
/// All public positions refer to play order.
#[derive(Debug, Clone)]
pub struct PlayQueue {
    items: Vec<TrackId>,
    order: Vec<usize>,
    cursor: Option<usize>,
    shuffle: bool,
    repeat: RepeatMode,
    rng: fastrand::Rng,
}

impl Default for PlayQueue {
    fn default() -> Self {
        Self::new(false, RepeatMode::Off)
    }
}

impl PlayQueue {
    pub fn new(shuffle: bool, repeat: RepeatMode) -> Self {
        Self::with_rng(shuffle, repeat, fastrand::Rng::new())
    }

    /// A queue with deterministic shuffling.
    #[cfg(test)]
    fn with_seed(shuffle: bool, repeat: RepeatMode, seed: u64) -> Self {
        Self::with_rng(shuffle, repeat, fastrand::Rng::with_seed(seed))
    }

    fn with_rng(shuffle: bool, repeat: RepeatMode, rng: fastrand::Rng) -> Self {
        Self {
            items: Vec::new(),
            order: Vec::new(),
            cursor: None,
            shuffle,
            repeat,
            rng,
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn shuffle(&self) -> bool {
        self.shuffle
    }

    pub fn repeat(&self) -> RepeatMode {
        self.repeat
    }

    /// Position of the current track in play order.
    pub fn position(&self) -> Option<usize> {
        self.cursor
    }

    pub fn current(&self) -> Option<TrackId> {
        self.cursor.map(|cursor| self.track_at(cursor))
    }

    pub fn get(&self, position: usize) -> Option<TrackId> {
        (position < self.order.len()).then(|| self.track_at(position))
    }

    /// Every track in play order.
    pub fn tracks(&self) -> Vec<TrackId> {
        self.order.iter().map(|&index| self.items[index]).collect()
    }

    /// Every track in the order they were added, which is the play order unless
    /// shuffled. With [`Self::shuffled_order`] and [`Self::position`], it is what
    /// [`Self::restore`] needs to bring the queue back on a later run.
    pub fn items(&self) -> &[TrackId] {
        &self.items
    }

    /// The play order as indices into [`Self::items`] while shuffled. Empty while not,
    /// when it is simply their order.
    pub fn shuffled_order(&self) -> &[usize] {
        if self.shuffle { &self.order } else { &[] }
    }

    /// Fills the queue with what an earlier run had in it: `items`, `order` and
    /// `current` as [`Self::items`], [`Self::shuffled_order`] and [`Self::position`]
    /// gave them then. Tracks that have left the library since are `None`, and are
    /// dropped; if the current track is one of them, the next one left in play order
    /// takes its place. Returns whether the current track is the one it was.
    pub fn restore(
        &mut self,
        items: Vec<Option<TrackId>>,
        order: Vec<usize>,
        current: usize,
    ) -> bool {
        let len = items.len();
        let mut seen = vec![false; len];
        let fits = order.len() == len
            && order
                .iter()
                .all(|&index| index < len && !std::mem::replace(&mut seen[index], true));
        // Not shuffled, or the order isn't one of these items (a damaged file).
        let order = if fits { order } else { (0..len).collect() };

        // Where each item that stays ends up among the items.
        let mut places = 0..;
        let renumbered: Vec<Option<usize>> = items
            .iter()
            .map(|item| item.and_then(|_| places.next()))
            .collect();
        self.items = items.into_iter().flatten().collect();
        self.order = Vec::with_capacity(self.items.len());
        self.cursor = None;
        let mut same_track = false;
        for (position, index) in order.into_iter().enumerate() {
            let Some(index) = renumbered[index] else {
                continue;
            };
            if position >= current && self.cursor.is_none() {
                self.cursor = Some(self.order.len());
                same_track = position == current;
            }
            self.order.push(index);
        }
        // The current track and all after it were dropped: the last one left is.
        self.cursor = self.cursor.or(self.order.len().checked_sub(1));
        if !self.shuffle {
            // Shuffle was turned off since: the tracks stay in the order they played in.
            self.items = self.order.iter().map(|&index| self.items[index]).collect();
            self.order = (0..self.items.len()).collect();
        }
        same_track
    }

    /// Tracks after the current one, in play order.
    pub fn upcoming(&self) -> impl Iterator<Item = TrackId> + '_ {
        let start = self.cursor.map_or(self.order.len(), |cursor| cursor + 1);
        self.order[start..].iter().map(|&index| self.items[index])
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.order.clear();
        self.cursor = None;
    }

    /// Replaces the queue. `start` is an index into `tracks`; with shuffle on that track
    /// plays first and the rest are shuffled, and `None` picks a random first track.
    pub fn set_tracks(&mut self, tracks: Vec<TrackId>, start: Option<usize>) -> Option<TrackId> {
        self.items = tracks;
        let len = self.items.len();
        if len == 0 {
            self.clear();
            return None;
        }
        let start = start.filter(|&index| index < len);
        if self.shuffle {
            self.order = (0..len).collect();
            self.rng.shuffle(&mut self.order);
            if let Some(start) = start {
                let at = self
                    .order
                    .iter()
                    .position(|&index| index == start)
                    .unwrap_or(0);
                self.order.swap(0, at);
            }
            self.cursor = Some(0);
        } else {
            self.order = (0..len).collect();
            self.cursor = Some(start.unwrap_or(0));
        }
        self.current()
    }

    /// Play-order positions in the order playback reaches them after the current
    /// track, wrapping around when repeating. When a track finishes by itself
    /// repeat-one plays it again; when the user presses "next" (`skipping`) it doesn't.
    /// Callers pass over positions they can't play.
    pub fn following_positions(&self, skipping: bool) -> impl Iterator<Item = usize> + use<> {
        let cursor = self.cursor;
        let current = cursor.filter(|_| self.repeat == RepeatMode::One && !skipping);
        let after = cursor.map_or(0..0, |cursor| cursor + 1..self.len());
        let wrapped = match (cursor, self.repeat) {
            (Some(cursor), RepeatMode::All | RepeatMode::One) => 0..cursor + 1,
            _ => 0..0,
        };
        current.into_iter().chain(after).chain(wrapped)
    }

    /// The user pressed "previous". At the start of the queue this wraps when repeating
    /// and otherwise stays on the first track.
    pub fn skip_previous(&mut self) -> Option<TrackId> {
        let cursor = self.cursor?;
        let previous = if cursor > 0 {
            cursor - 1
        } else if self.repeat != RepeatMode::Off {
            self.len() - 1
        } else {
            0
        };
        self.jump_to(previous)
    }

    /// Makes the track at `position` current. Out-of-range positions are ignored.
    pub fn jump_to(&mut self, position: usize) -> Option<TrackId> {
        if position >= self.order.len() {
            return None;
        }
        self.cursor = Some(position);
        self.current()
    }

    pub fn set_repeat(&mut self, repeat: RepeatMode) {
        self.repeat = repeat;
    }

    /// Turning shuffle on keeps the current track playing and shuffles everything else
    /// after it. Turning it off restores the original order at the current track.
    pub fn set_shuffle(&mut self, enabled: bool) {
        if enabled == self.shuffle {
            return;
        }
        self.shuffle = enabled;
        let Some(cursor) = self.cursor else { return };
        let current = self.order[cursor];
        if enabled {
            let mut rest: Vec<usize> = (0..self.len()).filter(|&index| index != current).collect();
            self.rng.shuffle(&mut rest);
            self.order = std::iter::once(current).chain(rest).collect();
            self.cursor = Some(0);
        } else {
            self.order = (0..self.len()).collect();
            self.cursor = Some(current);
        }
    }

    /// Inserts tracks to play straight after the current one.
    pub fn play_next(&mut self, tracks: &[TrackId]) {
        let Some(cursor) = self.cursor else {
            self.enqueue(tracks);
            return;
        };
        if self.shuffle {
            let first_new = self.items.len();
            self.items.extend_from_slice(tracks);
            self.order
                .splice(cursor + 1..cursor + 1, first_new..self.items.len());
        } else {
            self.items
                .splice(cursor + 1..cursor + 1, tracks.iter().copied());
            // The order is the identity while shuffle is off, so it only grows.
            let len = self.order.len();
            self.order.extend(len..self.items.len());
        }
    }

    /// Appends tracks to the end of the queue.
    pub fn enqueue(&mut self, tracks: &[TrackId]) {
        if tracks.is_empty() {
            return;
        }
        let first_new = self.items.len();
        self.items.extend_from_slice(tracks);
        self.order.extend(first_new..self.items.len());
        self.cursor.get_or_insert(0);
    }

    /// Removes the track at `position`. Returns `true` if it was the current track, in
    /// which case the following track (if any) becomes current.
    pub fn remove(&mut self, position: usize) -> bool {
        let Some(cursor) = self.cursor else {
            return false;
        };
        if position >= self.order.len() {
            return false;
        }
        let removed = self.order.remove(position);
        self.items.remove(removed);
        for index in &mut self.order {
            if *index > removed {
                *index -= 1;
            }
        }
        let len = self.order.len();
        self.cursor = if len == 0 {
            None
        } else if position < cursor {
            Some(cursor - 1)
        } else {
            Some(cursor.min(len - 1))
        };
        position == cursor
    }

    /// Removes every track after the current one in play order, in one pass. The
    /// current track and the ones before it stay where they are.
    pub fn clear_upcoming(&mut self) {
        let Some(cursor) = self.cursor else { return };
        self.order.truncate(cursor + 1);
        // The kept items stay in the order they were added, renumbered from 0.
        let mut renumbered: Vec<Option<usize>> = vec![None; self.items.len()];
        for &index in &self.order {
            renumbered[index] = Some(0);
        }
        for (new_index, slot) in renumbered.iter_mut().flatten().enumerate() {
            *slot = new_index;
        }
        let mut slots = renumbered.iter();
        self.items
            .retain(|_| slots.next().is_some_and(Option::is_some));
        for index in &mut self.order {
            *index = renumbered[*index].expect("every item left in the play order is kept");
        }
    }

    /// Moves a track from one play-order position to another, keeping the current
    /// track current.
    pub fn move_track(&mut self, from: usize, to: usize) {
        let len = self.len();
        let Some(cursor) = self.cursor else { return };
        if from >= len || to >= len || from == to {
            return;
        }
        if self.shuffle {
            let index = self.order.remove(from);
            self.order.insert(to, index);
        } else {
            let track = self.items.remove(from);
            self.items.insert(to, track);
        }
        self.cursor = Some(if cursor == from {
            to
        } else if from < cursor && to >= cursor {
            cursor - 1
        } else if from > cursor && to <= cursor {
            cursor + 1
        } else {
            cursor
        });
    }

    fn track_at(&self, position: usize) -> TrackId {
        self.items[self.order[position]]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(range: std::ops::Range<i64>) -> Vec<TrackId> {
        range.map(TrackId).collect()
    }

    fn queue(len: i64, shuffle: bool, repeat: RepeatMode) -> PlayQueue {
        let mut queue = PlayQueue::with_seed(shuffle, repeat, 7);
        queue.set_tracks(ids(0..len), Some(0));
        queue
    }

    /// The track playback moves to next: by itself, or as the user presses "next"
    /// (`skipping`).
    fn next(queue: &PlayQueue, skipping: bool) -> Option<TrackId> {
        let position = queue.following_positions(skipping).next()?;
        queue.get(position)
    }

    fn move_next(queue: &mut PlayQueue, skipping: bool) -> Option<TrackId> {
        let position = queue.following_positions(skipping).next()?;
        queue.jump_to(position)
    }

    fn is_permutation(queue: &PlayQueue) -> bool {
        let mut tracks = queue.tracks();
        tracks.sort();
        tracks == {
            let mut items = queue.items.clone();
            items.sort();
            items
        }
    }

    /// A new queue with what `queue` would keep for a later run, leaving out `dropped`.
    fn restored(queue: &PlayQueue, shuffle: bool, dropped: &[i64]) -> (PlayQueue, bool) {
        let items = queue
            .items()
            .iter()
            .map(|track| (!dropped.contains(&track.0)).then_some(*track))
            .collect();
        let order = queue.shuffled_order().to_vec();
        let mut later = PlayQueue::with_seed(shuffle, queue.repeat(), 1);
        let same_track = later.restore(items, order, queue.position().unwrap_or(0));
        (later, same_track)
    }

    #[test]
    fn a_queue_comes_back_as_it_was() {
        let mut q = queue(5, false, RepeatMode::Off);
        q.jump_to(3);
        let (later, same_track) = restored(&q, false, &[]);
        assert!(same_track);
        assert_eq!(later.tracks(), q.tracks());
        assert_eq!(later.current(), Some(TrackId(3)));

        let mut q = queue(6, true, RepeatMode::All);
        q.jump_to(2);
        let (mut later, same_track) = restored(&q, true, &[]);
        assert!(same_track);
        assert_eq!(later.tracks(), q.tracks());
        assert_eq!(later.current(), q.current());
        // Turning shuffle off still finds the order the tracks were added in.
        later.set_shuffle(false);
        assert_eq!(later.tracks(), ids(0..6));
        assert_eq!(later.current(), q.current());
    }

    #[test]
    fn tracks_that_left_the_library_are_dropped_from_a_restored_queue() {
        let mut q = queue(5, false, RepeatMode::Off);
        q.jump_to(2);
        let (later, same_track) = restored(&q, false, &[0, 4]);
        assert!(same_track);
        assert_eq!(later.tracks(), ids(1..4));
        assert_eq!(later.current(), Some(TrackId(2)));

        // Without the current track, the one after it is current.
        let (later, same_track) = restored(&q, false, &[2]);
        assert!(!same_track);
        assert_eq!(later.current(), Some(TrackId(3)));
        let (later, same_track) = restored(&q, false, &[2, 3, 4]);
        assert!(!same_track);
        assert_eq!(later.current(), Some(TrackId(1)));
        let (later, same_track) = restored(&q, false, &[0, 1, 2, 3, 4]);
        assert!(!same_track && later.is_empty() && later.position().is_none());

        let mut q = queue(6, true, RepeatMode::Off);
        q.jump_to(3);
        let current = q.current().unwrap();
        let gone = q.get(1).unwrap();
        let left: Vec<TrackId> = q.tracks().into_iter().filter(|&id| id != gone).collect();
        let (later, same_track) = restored(&q, true, &[gone.0]);
        assert!(same_track);
        assert_eq!(later.tracks(), left);
        assert_eq!(later.current(), Some(current));
        assert!(is_permutation(&later));
    }

    #[test]
    fn a_restored_queue_fits_the_shuffle_setting_and_survives_a_damaged_order() {
        // Shuffled when it was kept, but shuffle is off now.
        let mut q = queue(5, true, RepeatMode::Off);
        q.jump_to(1);
        let (mut later, same_track) = restored(&q, false, &[]);
        assert!(same_track);
        assert_eq!(later.tracks(), q.tracks());
        assert_eq!(later.items(), q.tracks());
        assert!(later.shuffled_order().is_empty());
        later.play_next(&[TrackId(9)]);
        assert_eq!(later.get(2), Some(TrackId(9)));

        for order in [vec![0, 0, 1], vec![0, 1], vec![0, 1, 7]] {
            let mut later = PlayQueue::with_seed(true, RepeatMode::Off, 1);
            let items = ids(0..3).into_iter().map(Some).collect();
            assert!(later.restore(items, order, 1));
            assert_eq!(later.tracks(), ids(0..3));
            assert_eq!(later.current(), Some(TrackId(1)));
        }
        let mut later = PlayQueue::default();
        let items = ids(0..2).into_iter().map(Some).collect();
        assert!(!later.restore(items, Vec::new(), 9));
        assert_eq!(later.current(), Some(TrackId(1)));
    }

    #[test]
    fn plays_in_order_and_stops_at_end() {
        let mut q = queue(3, false, RepeatMode::Off);
        assert_eq!(q.current(), Some(TrackId(0)));
        assert_eq!(next(&q, false), Some(TrackId(1)));
        assert_eq!(move_next(&mut q, false), Some(TrackId(1)));
        assert_eq!(move_next(&mut q, false), Some(TrackId(2)));
        assert_eq!(next(&q, false), None);
        assert_eq!(move_next(&mut q, false), None);
        assert_eq!(q.current(), Some(TrackId(2)));
    }

    #[test]
    fn repeat_modes() {
        let mut q = queue(2, false, RepeatMode::All);
        q.jump_to(1);
        assert_eq!(next(&q, false), Some(TrackId(0)));
        assert_eq!(move_next(&mut q, false), Some(TrackId(0)));

        q.set_repeat(RepeatMode::One);
        assert_eq!(next(&q, false), Some(TrackId(0)));
        assert_eq!(move_next(&mut q, false), Some(TrackId(0)));
        assert_eq!(move_next(&mut q, true), Some(TrackId(1)));
        assert_eq!(move_next(&mut q, true), Some(TrackId(0)));
        assert_eq!(q.skip_previous(), Some(TrackId(1)));
    }

    #[test]
    fn following_positions_wrap_only_when_repeating() {
        let walk = |len, position, repeat, skipping| {
            let mut q = queue(len, false, repeat);
            q.jump_to(position);
            q.following_positions(skipping).collect::<Vec<_>>()
        };
        assert_eq!(walk(4, 1, RepeatMode::Off, false), [2, 3]);
        assert!(walk(4, 3, RepeatMode::Off, false).is_empty());
        assert_eq!(walk(3, 1, RepeatMode::All, false), [2, 0, 1]);
        assert_eq!(walk(3, 1, RepeatMode::One, false), [1, 2, 0, 1]);
        assert_eq!(walk(3, 1, RepeatMode::One, true), [2, 0, 1]);
        assert_eq!(PlayQueue::default().following_positions(false).count(), 0);
    }

    #[test]
    fn previous_at_start_without_repeat_stays() {
        let mut q = queue(3, false, RepeatMode::Off);
        assert_eq!(q.skip_previous(), Some(TrackId(0)));
        assert_eq!(move_next(&mut q, true), Some(TrackId(1)));
        assert_eq!(q.skip_previous(), Some(TrackId(0)));
    }

    #[test]
    fn shuffle_keeps_current_first_and_covers_everything() {
        let mut q = PlayQueue::with_seed(true, RepeatMode::Off, 42);
        q.set_tracks(ids(0..50), Some(10));
        assert_eq!(q.current(), Some(TrackId(10)));
        assert_eq!(q.position(), Some(0));
        assert!(is_permutation(&q));
        assert_ne!(q.tracks(), {
            let mut v = ids(0..50);
            v.swap(0, 10);
            v
        });
    }

    #[test]
    fn toggling_shuffle_preserves_current() {
        let mut q = queue(20, false, RepeatMode::Off);
        q.jump_to(5);
        q.set_shuffle(true);
        assert_eq!(q.current(), Some(TrackId(5)));
        assert_eq!(q.position(), Some(0));
        assert_eq!(q.upcoming().count(), 19);
        move_next(&mut q, true);
        let now = q.current().unwrap();
        q.set_shuffle(false);
        assert_eq!(q.current(), Some(now));
        assert_eq!(q.tracks(), ids(0..20));
        assert_eq!(q.position(), Some(now.0 as usize));
    }

    #[test]
    fn random_start_when_shuffling_without_start() {
        let mut q = PlayQueue::with_seed(true, RepeatMode::Off, 1);
        assert!(q.set_tracks(ids(0..10), None).is_some());
        assert_eq!(q.len(), 10);
        assert!(is_permutation(&q));
    }

    #[test]
    fn play_next_and_enqueue() {
        let mut q = queue(3, false, RepeatMode::Off);
        q.play_next(&[TrackId(100), TrackId(101)]);
        q.enqueue(&[TrackId(200)]);
        assert_eq!(
            q.tracks(),
            vec![
                TrackId(0),
                TrackId(100),
                TrackId(101),
                TrackId(1),
                TrackId(2),
                TrackId(200)
            ]
        );

        let mut q = queue(5, true, RepeatMode::Off);
        q.play_next(&[TrackId(100)]);
        assert_eq!(next(&q, false), Some(TrackId(100)));
        assert!(is_permutation(&q));

        let mut empty = PlayQueue::default();
        empty.play_next(&[TrackId(9)]);
        assert_eq!(empty.current(), Some(TrackId(9)));
    }

    #[test]
    fn remove_adjusts_cursor() {
        let mut q = queue(4, false, RepeatMode::Off);
        q.jump_to(2);
        assert!(!q.remove(0));
        assert_eq!(q.current(), Some(TrackId(2)));
        assert_eq!(q.position(), Some(1));
        assert!(q.remove(1));
        assert_eq!(q.current(), Some(TrackId(3)));
        assert!(q.remove(1));
        assert_eq!(q.current(), Some(TrackId(1)));
        assert!(q.remove(0));
        assert!(q.is_empty());
        assert_eq!(q.current(), None);
    }

    #[test]
    fn remove_in_shuffle_keeps_permutation() {
        let mut q = queue(10, true, RepeatMode::Off);
        let current = q.current();
        q.remove(3);
        q.remove(5);
        assert_eq!(q.len(), 8);
        assert_eq!(q.current(), current);
        assert!(is_permutation(&q));
    }

    #[test]
    fn clear_upcoming_keeps_current_and_played() {
        let mut q = queue(5, false, RepeatMode::Off);
        q.jump_to(2);
        q.clear_upcoming();
        assert_eq!(q.tracks(), ids(0..3));
        assert_eq!(q.current(), Some(TrackId(2)));
        assert_eq!(next(&q, false), None);

        // At the last track there is nothing to remove.
        q.clear_upcoming();
        assert_eq!(q.tracks(), ids(0..3));

        let mut empty = PlayQueue::default();
        empty.clear_upcoming();
        assert!(empty.is_empty());
    }

    #[test]
    fn clear_upcoming_while_shuffled() {
        let mut q = queue(10, true, RepeatMode::Off);
        move_next(&mut q, true);
        move_next(&mut q, true);
        let played = q.tracks()[..3].to_vec();
        let current = q.current();
        q.clear_upcoming();
        assert_eq!(q.tracks(), played);
        assert_eq!(q.current(), current);
        assert_eq!(q.position(), Some(2));
        assert!(is_permutation(&q));

        // Turning shuffle off puts what is left back in the order it was added.
        q.set_shuffle(false);
        let mut added = played.clone();
        added.sort();
        assert_eq!(q.tracks(), added);
        assert_eq!(q.current(), current);
    }

    #[test]
    fn move_track_keeps_current() {
        let mut q = queue(5, false, RepeatMode::Off);
        q.jump_to(2);
        q.move_track(0, 4);
        assert_eq!(q.current(), Some(TrackId(2)));
        assert_eq!(
            q.tracks(),
            ids(1..5)
                .into_iter()
                .chain([TrackId(0)])
                .collect::<Vec<_>>()
        );
        q.move_track(q.position().unwrap(), 0);
        assert_eq!(q.position(), Some(0));
        assert_eq!(q.current(), Some(TrackId(2)));
    }

    #[test]
    fn repeat_cycles() {
        assert_eq!(RepeatMode::Off.cycle(), RepeatMode::All);
        assert_eq!(RepeatMode::All.cycle(), RepeatMode::One);
        assert_eq!(RepeatMode::One.cycle(), RepeatMode::Off);
        assert_eq!(RepeatMode::from_id("one"), Some(RepeatMode::One));
    }
}
