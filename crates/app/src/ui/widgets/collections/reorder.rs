//! Turning a drop in a reorderable list into the new order of a playlist, or into a
//! move within the play queue.

use std::collections::HashSet;

use mp_core::PlaylistItemId;

/// The playlist's order after dropping the `dragged` entries, in the order given,
/// before the shown row `before`: right before the first shown row from there on
/// that isn't being dragged, or at the end.
///
/// `order` is the whole playlist; `rows` are the entries the list shows, which can
/// leave some out (files that are missing). Missing entries keep their place
/// relative to the entries around them.
pub fn drop_order(
    order: &[PlaylistItemId],
    rows: &[PlaylistItemId],
    dragged: &[PlaylistItemId],
    before: usize,
) -> Vec<PlaylistItemId> {
    let is_dragged: HashSet<&PlaylistItemId> = dragged.iter().collect();
    let anchor = rows
        .get(before..)
        .and_then(|rest| rest.iter().find(|item| !is_dragged.contains(item)));
    let mut target: Vec<PlaylistItemId> = order
        .iter()
        .filter(|item| !is_dragged.contains(item))
        .copied()
        .collect();
    let at = anchor
        .and_then(|anchor| target.iter().position(|item| item == anchor))
        .unwrap_or(target.len());
    target.splice(at..at, dragged.iter().copied());
    target
}

/// The playlist's order with removed entries put back where they were: `added` are
/// the entries just appended for them, `indices` their old positions in ascending
/// order.
pub fn restored_order(
    order: &[PlaylistItemId],
    added: &[PlaylistItemId],
    indices: &[usize],
) -> Vec<PlaylistItemId> {
    let is_added: HashSet<&PlaylistItemId> = added.iter().collect();
    let mut restored: Vec<PlaylistItemId> = order
        .iter()
        .filter(|item| !is_added.contains(item))
        .copied()
        .collect();
    // Placing the first one first leaves every entry before the next one in place.
    for (&item, &index) in added.iter().zip(indices) {
        restored.insert(index.min(restored.len()), item);
    }
    restored
}

/// Where `Player::move_in_queue` must move the track at `from` for it to end up just
/// before the track now at `before`. `None` if it is already there.
pub fn queue_move(from: usize, before: usize) -> Option<usize> {
    let to = if before > from { before - 1 } else { before };
    (to != from).then_some(to)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(ids: &[i64]) -> Vec<PlaylistItemId> {
        ids.iter().copied().map(PlaylistItemId).collect()
    }

    fn ids(items: &[PlaylistItemId]) -> Vec<i64> {
        items.iter().map(|item| item.0).collect()
    }

    #[test]
    fn dropped_rows_move_as_a_block() {
        let order = items(&[1, 2, 3, 4, 5]);
        let drop = |dragged: &[i64], before: usize| {
            ids(&drop_order(&order, &order, &items(dragged), before))
        };
        assert_eq!(drop(&[1], 5), [2, 3, 4, 5, 1]);
        assert_eq!(drop(&[5], 0), [5, 1, 2, 3, 4]);
        assert_eq!(drop(&[1, 3], 5), [2, 4, 5, 1, 3]);
        assert_eq!(drop(&[2, 5], 1), [1, 2, 5, 3, 4]);
        assert_eq!(drop(&[1, 5], 2), [2, 1, 5, 3, 4]);
        assert_eq!(drop(&[2, 3, 5], 3), [1, 2, 3, 5, 4]);
        // Dropping rows onto themselves changes nothing.
        assert_eq!(drop(&[2, 3], 2), [1, 2, 3, 4, 5]);
        assert_eq!(drop(&[2, 3], 3), [1, 2, 3, 4, 5]);
        assert_eq!(drop(&[], 1), [1, 2, 3, 4, 5]);
        assert_eq!(drop(&[1, 2, 3, 4], 5), [5, 1, 2, 3, 4]);
    }

    #[test]
    fn missing_entries_keep_their_place() {
        // Entry 9 is missing, so the list shows 1, 2, 3 while the playlist is 1, 9, 2, 3.
        let order = items(&[1, 9, 2, 3]);
        let rows = items(&[1, 2, 3]);
        assert_eq!(
            ids(&drop_order(&order, &rows, &items(&[3]), 1)),
            [1, 9, 3, 2]
        );
        assert_eq!(
            ids(&drop_order(&order, &rows, &items(&[1]), 3)),
            [9, 2, 3, 1]
        );
    }

    #[test]
    fn restoring_puts_removed_entries_back() {
        // 2 and 4 were removed from 1..=5, then appended again as 6 and 7.
        let order = items(&[1, 3, 5, 6, 7]);
        let restored = restored_order(&order, &items(&[6, 7]), &[1, 3]);
        assert_eq!(ids(&restored), [1, 6, 3, 7, 5]);
        // Entries removed from the end go back to the end.
        let restored = restored_order(&items(&[1, 2, 3]), &items(&[3]), &[7]);
        assert_eq!(ids(&restored), [1, 2, 3]);
    }

    #[test]
    fn queue_moves_target_the_slot_before_the_drop_row() {
        // Down: from 2 to before 5 means index 4 once 2 has been taken out.
        assert_eq!(queue_move(2, 5), Some(4));
        assert_eq!(queue_move(5, 2), Some(2));
        // Dropping a row just before or after itself changes nothing.
        assert_eq!(queue_move(3, 3), None);
        assert_eq!(queue_move(3, 4), None);
    }
}
