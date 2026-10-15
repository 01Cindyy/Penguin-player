use std::fs;
use std::path::PathBuf;

use mp_core::{PlaylistId, PlaylistItemId, TrackId};

use super::fixtures::{TestLibrary, TestTags, write_wav};
use crate::LibraryError;

/// A library with three scanned tracks, returned in path order.
fn library_with_tracks() -> (TestLibrary, Vec<(PathBuf, TrackId)>) {
    let mut lib = TestLibrary::new();
    let paths: Vec<PathBuf> = ["a.wav", "b.wav", "c.wav"]
        .into_iter()
        .map(|name| lib.path(name))
        .collect();
    for path in &paths {
        write_wav(path, &TestTags::default());
    }
    lib.scan();
    let tracks = paths
        .into_iter()
        .map(|path| {
            let id = lib.track_at(&path).unwrap().id;
            (path, id)
        })
        .collect();
    (lib, tracks)
}

fn entry_tracks(lib: &TestLibrary, id: PlaylistId) -> Vec<Option<TrackId>> {
    let playlist = lib.library.playlist(id).unwrap().unwrap();
    playlist.entries.iter().map(|entry| entry.track).collect()
}

fn item_ids(lib: &TestLibrary, id: PlaylistId) -> Vec<PlaylistItemId> {
    let playlist = lib.library.playlist(id).unwrap().unwrap();
    playlist.entries.iter().map(|entry| entry.item).collect()
}

/// Stored positions in playlist order, which must always be 0, 1, 2, ...
fn positions(lib: &TestLibrary, id: PlaylistId) -> Vec<i64> {
    let mut statement = lib
        .library
        .conn
        .prepare("SELECT position FROM playlist_items WHERE playlist_id = ?1 ORDER BY position")
        .unwrap();
    statement
        .query_map([id.0], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

#[test]
fn create_rename_and_delete() {
    let lib = TestLibrary::new();
    let road = lib.library.create_playlist("  Road Trip  ").unwrap();
    let chill = lib.library.create_playlist("chill").unwrap();
    let accent = lib.library.create_playlist("Ápres").unwrap();

    let names: Vec<String> = lib
        .library
        .playlists()
        .unwrap()
        .into_iter()
        .map(|playlist| playlist.name)
        .collect();
    assert_eq!(names, ["Ápres", "chill", "Road Trip"]);

    let playlist = lib.library.playlist(road).unwrap().unwrap();
    assert!(playlist.entries.is_empty());
    assert!(playlist.created > 0);

    lib.library.rename_playlist(chill, " Evening ").unwrap();
    assert_eq!(
        lib.library.playlist(chill).unwrap().unwrap().name,
        "Evening"
    );

    lib.library.delete_playlist(accent).unwrap();
    assert_eq!(lib.library.playlist(accent).unwrap(), None);
    assert_eq!(lib.library.playlists().unwrap().len(), 2);
}

#[test]
fn invalid_names_and_missing_playlists_are_rejected() {
    let lib = TestLibrary::new();
    let id = lib.library.create_playlist("Mix").unwrap();
    let missing = PlaylistId(id.0 + 100);

    assert!(matches!(
        lib.library.create_playlist("   "),
        Err(LibraryError::Invalid(_))
    ));
    assert!(matches!(
        lib.library.rename_playlist(id, ""),
        Err(LibraryError::Invalid(_))
    ));
    assert!(matches!(
        lib.library.rename_playlist(missing, "x"),
        Err(LibraryError::Invalid(_))
    ));
    assert!(matches!(
        lib.library.delete_playlist(missing),
        Err(LibraryError::Invalid(_))
    ));
    assert!(matches!(
        lib.library.add_to_playlist(missing, &[]),
        Err(LibraryError::Invalid(_))
    ));
    assert_eq!(lib.library.playlist(id).unwrap().unwrap().name, "Mix");
}

#[test]
fn add_remove_and_reorder_keep_positions_contiguous() {
    let (lib, tracks) = library_with_tracks();
    let [a, b, c] = [tracks[0].1, tracks[1].1, tracks[2].1];
    let id = lib.library.create_playlist("Mix").unwrap();

    lib.library.add_to_playlist(id, &[a, b]).unwrap();
    lib.library.add_to_playlist(id, &[c, a]).unwrap();
    assert_eq!(entry_tracks(&lib, id), [Some(a), Some(b), Some(c), Some(a)]);
    assert_eq!(positions(&lib, id), [0, 1, 2, 3]);

    let items = item_ids(&lib, id);
    lib.library.remove_from_playlist(id, &[items[1]]).unwrap();
    assert_eq!(entry_tracks(&lib, id), [Some(a), Some(c), Some(a)]);
    assert_eq!(positions(&lib, id), [0, 1, 2]);

    // Move the first entry to the end, then back to the front.
    let items = item_ids(&lib, id);
    let moved = [items[1], items[2], items[0]];
    lib.library.reorder_playlist(id, &moved).unwrap();
    assert_eq!(item_ids(&lib, id), moved);
    lib.library.reorder_playlist(id, &items).unwrap();
    assert_eq!(item_ids(&lib, id), items);
    assert_eq!(positions(&lib, id), [0, 1, 2]);

    // Unknown items and tracks are errors that change nothing.
    let before = item_ids(&lib, id);
    assert!(matches!(
        lib.library
            .reorder_playlist(id, &[before[0], before[1], PlaylistItemId(9_999)]),
        Err(LibraryError::Invalid(_))
    ));
    assert!(matches!(
        lib.library.add_to_playlist(id, &[a, TrackId(9_999)]),
        Err(LibraryError::Invalid(_))
    ));
    assert_eq!(item_ids(&lib, id), before);

    lib.library.remove_from_playlist(id, &before).unwrap();
    assert!(item_ids(&lib, id).is_empty());
}

#[test]
fn changes_update_the_modified_time() {
    let (lib, tracks) = library_with_tracks();
    let id = lib.library.create_playlist("Mix").unwrap();
    let reset = || {
        lib.library
            .conn
            .execute("UPDATE playlists SET modified = 0", [])
            .unwrap();
    };
    let modified = || lib.library.playlist(id).unwrap().unwrap().modified;

    reset();
    lib.library.add_to_playlist(id, &[tracks[0].1]).unwrap();
    assert!(modified() > 0);

    reset();
    let item = item_ids(&lib, id)[0];
    lib.library.reorder_playlist(id, &[item]).unwrap();
    assert!(modified() > 0);

    reset();
    lib.library.remove_from_playlist(id, &[item]).unwrap();
    assert!(modified() > 0);

    reset();
    lib.library.rename_playlist(id, "Renamed").unwrap();
    assert!(modified() > 0);
}

#[test]
fn entries_survive_tracks_disappearing_and_returning() {
    let (mut lib, tracks) = library_with_tracks();
    let (path, track) = tracks[1].clone();
    let id = lib.library.create_playlist("Mix").unwrap();
    lib.library.add_to_playlist(id, &[track]).unwrap();

    let moved_away = lib.root.join("../elsewhere.wav");
    fs::rename(&path, &moved_away).unwrap();
    lib.scan();
    let playlist = lib.library.playlist(id).unwrap().unwrap();
    assert_eq!(playlist.entries.len(), 1);
    assert_eq!(playlist.entries[0].path, path);
    assert_eq!(playlist.entries[0].track, None);
    assert_eq!(playlist.available_tracks().count(), 0);

    fs::rename(&moved_away, &path).unwrap();
    lib.scan();
    let returned = lib.track_at(&path).unwrap().id;
    assert_eq!(entry_tracks(&lib, id), [Some(returned)]);
}

#[test]
fn deleting_a_playlist_deletes_its_items() {
    let (lib, tracks) = library_with_tracks();
    let id = lib.library.create_playlist("Mix").unwrap();
    lib.library.add_to_playlist(id, &[tracks[0].1]).unwrap();
    lib.library.delete_playlist(id).unwrap();

    let items: i64 = lib
        .library
        .conn
        .query_row("SELECT COUNT(*) FROM playlist_items", [], |row| row.get(0))
        .unwrap();
    assert_eq!(items, 0);
}

#[test]
fn in_memory_libraries_work() {
    let library = crate::Library::open_in_memory().unwrap();
    assert_eq!(library.track_count().unwrap(), 0);
    assert!(library.tracks().unwrap().is_empty());
    let id = library.create_playlist("Mix").unwrap();
    assert_eq!(library.playlists().unwrap()[0].id, id);
}

#[test]
fn every_playlist_gets_its_own_entries() {
    let (lib, tracks) = library_with_tracks();
    let [a, b, c] = [tracks[0].1, tracks[1].1, tracks[2].1];
    let first = lib.library.create_playlist("First").unwrap();
    let second = lib.library.create_playlist("Second").unwrap();
    let empty = lib.library.create_playlist("Empty").unwrap();
    lib.library.add_to_playlist(second, &[c, a]).unwrap();
    lib.library.add_to_playlist(first, &[b]).unwrap();

    let playlists = lib.library.playlists().unwrap();
    for playlist in &playlists {
        assert_eq!(
            Some(playlist),
            lib.library.playlist(playlist.id).unwrap().as_ref()
        );
    }
    let tracks_of = |id| {
        let playlist = playlists.iter().find(|playlist| playlist.id == id).unwrap();
        playlist.available_tracks().collect::<Vec<_>>()
    };
    assert_eq!(tracks_of(first), [b]);
    assert_eq!(tracks_of(second), [c, a]);
    assert!(tracks_of(empty).is_empty());
}

#[test]
fn duplicates_are_separate_entries() {
    let (lib, tracks) = library_with_tracks();
    let [a, b] = [tracks[0].1, tracks[1].1];
    let id = lib.library.create_playlist("Mix").unwrap();
    lib.library.add_to_playlist(id, &[a, b, a]).unwrap();
    let items = item_ids(&lib, id);

    lib.library.remove_from_playlist(id, &[items[0]]).unwrap();
    assert_eq!(item_ids(&lib, id), [items[1], items[2]]);
    assert_eq!(entry_tracks(&lib, id), [Some(b), Some(a)]);
    assert_eq!(positions(&lib, id), [0, 1]);
}

#[test]
fn reorder_saves_a_whole_new_order() {
    let (lib, tracks) = library_with_tracks();
    let [a, b, c] = [tracks[0].1, tracks[1].1, tracks[2].1];
    let id = lib.library.create_playlist("Mix").unwrap();
    lib.library.add_to_playlist(id, &[a, b, c, a]).unwrap();
    let items = item_ids(&lib, id);

    let reversed: Vec<PlaylistItemId> = items.iter().rev().copied().collect();
    lib.library.reorder_playlist(id, &reversed).unwrap();
    assert_eq!(item_ids(&lib, id), reversed);
    assert_eq!(entry_tracks(&lib, id), [Some(a), Some(c), Some(b), Some(a)]);
    assert_eq!(positions(&lib, id), [0, 1, 2, 3]);

    lib.library.reorder_playlist(id, &items).unwrap();
    assert_eq!(item_ids(&lib, id), items);
}

#[test]
fn reorder_rejects_anything_but_a_permutation() {
    let (lib, tracks) = library_with_tracks();
    let id = lib.library.create_playlist("Mix").unwrap();
    let ids: Vec<TrackId> = tracks.iter().map(|(_, id)| *id).collect();
    lib.library.add_to_playlist(id, &ids).unwrap();
    let items = item_ids(&lib, id);
    let other = lib.library.create_playlist("Other").unwrap();
    lib.library.add_to_playlist(other, &ids[..1]).unwrap();
    let foreign = item_ids(&lib, other)[0];

    let invalid = [
        vec![items[0], items[1]],
        vec![items[0], items[1], items[1]],
        vec![items[0], items[1], items[2], items[0]],
        vec![items[0], items[1], foreign],
        vec![items[0], items[1], items[2], PlaylistItemId(9_999)],
    ];
    for order in invalid {
        assert!(
            matches!(
                lib.library.reorder_playlist(id, &order),
                Err(LibraryError::Invalid(_))
            ),
            "{order:?} was accepted"
        );
        assert_eq!(item_ids(&lib, id), items);
    }
    assert!(matches!(
        lib.library.reorder_playlist(PlaylistId(9_999), &[]),
        Err(LibraryError::Invalid(_))
    ));

    let empty = lib.library.create_playlist("Empty").unwrap();
    lib.library.reorder_playlist(empty, &[]).unwrap();
}
