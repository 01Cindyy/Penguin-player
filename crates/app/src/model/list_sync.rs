//! Updating a `gio::ListStore` with the smallest single splice, so the views on top
//! of it keep their scroll position, focus and selection wherever nothing changed.

use gtk::prelude::*;
use gtk::{gio, glib};

/// The part of a list that differs between two versions: `removed` items starting
/// at `position` in the old version were replaced by `added` items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Change {
    pub position: usize,
    pub removed: usize,
    pub added: usize,
}

/// Compares two versions of a list by their common start and end. `None` when they
/// are equal. Linear, so it is cheap even for lists of many thousands.
pub fn changed_range<T: PartialEq>(old: &[T], new: &[T]) -> Option<Change> {
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    if prefix == old.len() && prefix == new.len() {
        return None;
    }
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    Some(Change {
        position: prefix,
        removed: old.len() - prefix - suffix,
        added: new.len() - prefix - suffix,
    })
}

/// Makes `store` hold exactly `items`, in order.
pub fn sync_list_store<O: IsA<glib::Object>>(store: &gio::ListStore, items: &[O]) {
    let old: Vec<glib::Object> = (0..store.n_items())
        .filter_map(|position| store.item(position))
        .collect();
    let new: Vec<glib::Object> = items.iter().map(|item| item.clone().upcast()).collect();
    if let Some(change) = changed_range(&old, &new) {
        let added = &new[change.position..change.position + change.added];
        store.splice(change.position as u32, change.removed as u32, added);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_changed_middle() {
        assert_eq!(changed_range(&[1, 2, 3], &[1, 2, 3]), None);
        let change = |position, removed, added| {
            Some(Change {
                position,
                removed,
                added,
            })
        };
        assert_eq!(changed_range(&[1, 2, 3], &[2, 3]), change(0, 1, 0));
        assert_eq!(changed_range(&[1, 2, 3], &[1, 3, 2]), change(1, 2, 2));
        assert_eq!(changed_range(&[1, 2], &[1, 2, 9]), change(2, 0, 1));
        assert_eq!(changed_range(&[], &[4, 5]), change(0, 0, 2));
        // Repeated items never make the common start and end overlap.
        assert_eq!(changed_range(&[7, 7], &[7, 7, 7]), change(2, 0, 1));
        assert_eq!(changed_range(&[7, 7, 7], &[7]), change(1, 2, 0));
    }

    #[test]
    fn splices_only_what_changed() {
        let objects: Vec<glib::Object> = (0..4).map(|_| glib::Object::new()).collect();
        let store = gio::ListStore::new::<glib::Object>();
        sync_list_store(&store, &objects);
        assert_eq!(store.n_items(), 4);

        let changes = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let log = changes.clone();
        store.connect_items_changed(move |_, position, removed, added| {
            log.borrow_mut().push((position, removed, added));
        });
        let moved = [&objects[0], &objects[2], &objects[1], &objects[3]].map(Clone::clone);
        sync_list_store(&store, &moved);
        sync_list_store(&store, &moved);
        assert_eq!(*changes.borrow(), vec![(1, 2, 2)]);
        assert_eq!(store.item(1).as_ref(), Some(&objects[2]));
    }
}
