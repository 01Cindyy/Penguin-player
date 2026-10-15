//! Tests that need the crate's internals. They generate their own audio files, so they
//! don't depend on anything outside a temporary folder.

mod bench;
mod changes;
mod fixtures;
mod id3;
mod lyrics;
mod playlists;
mod robustness;
mod scan;
mod watcher;

/// The app opens a `Library` on a worker thread and keeps the watcher in its context.
#[test]
fn handles_can_move_between_threads() {
    fn assert_send<T: Send>() {}
    assert_send::<crate::Library>();
    assert_send::<crate::LibraryWatcher>();
}
