use std::fs;
use std::path::PathBuf;

use mp_core::{CoverId, TrackId};

use super::fixtures::{TestLibrary, TestTags, write_wav};

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

fn favourite_tracks(lib: &TestLibrary) -> Vec<Option<TrackId>> {
    let favourites = lib.library.favourites().unwrap();
    favourites.entries.iter().map(|entry| entry.track).collect()
}

#[test]
fn newest_come_first_and_songs_added_together_keep_their_order() {
    let (lib, tracks) = library_with_tracks();
    let [a, b, c] = [tracks[0].1, tracks[1].1, tracks[2].1];
    lib.library.add_favourites(&[a]).unwrap();
    lib.library.add_favourites(&[b, c]).unwrap();
    assert_eq!(favourite_tracks(&lib), [Some(b), Some(c), Some(a)]);

    // A song that already is one keeps its place, and unknown ids are skipped.
    lib.library.add_favourites(&[a, TrackId(999)]).unwrap();
    assert_eq!(favourite_tracks(&lib), [Some(b), Some(c), Some(a)]);
    let entries = lib.library.favourites().unwrap().entries;
    assert_eq!(entries[2].path, tracks[0].0);
    assert!(entries.windows(2).all(|pair| pair[0].added > pair[1].added));
}

#[test]
fn removed_favourites_can_be_put_back_in_their_place() {
    let (lib, tracks) = library_with_tracks();
    let ids: Vec<TrackId> = tracks.iter().map(|(_, id)| *id).collect();
    lib.library.add_favourites(&ids).unwrap();
    let before = lib.library.favourites().unwrap().entries;

    lib.library
        .remove_favourites(&[tracks[1].0.clone(), lib.path("not-a-favourite.wav")])
        .unwrap();
    assert_eq!(favourite_tracks(&lib), [Some(ids[0]), Some(ids[2])]);

    lib.library.restore_favourites(&before[1..2]).unwrap();
    assert_eq!(lib.library.favourites().unwrap().entries, before);
}

#[test]
fn favourites_survive_files_disappearing_and_returning() {
    let (mut lib, tracks) = library_with_tracks();
    let (path, track) = tracks[1].clone();
    lib.library.add_favourites(&[track]).unwrap();

    let moved_away = lib.root.join("../elsewhere.wav");
    fs::rename(&path, &moved_away).unwrap();
    lib.scan();
    let favourites = lib.library.favourites().unwrap();
    assert_eq!(favourites.entries.len(), 1);
    assert_eq!(favourites.entries[0].path, path);
    assert_eq!(favourites.available_tracks().count(), 0);

    fs::rename(&moved_away, &path).unwrap();
    lib.scan();
    let returned = lib.track_at(&path).unwrap().id;
    assert_eq!(favourite_tracks(&lib), [Some(returned)]);
}

#[test]
fn favourites_follow_moved_folders() {
    let mut lib = TestLibrary::new();
    let before = lib.path("Old Name/01.wav");
    write_wav(&before, &TestTags::default());
    lib.scan();
    let track = lib.track_at(&before).unwrap().id;
    lib.library.add_favourites(&[track]).unwrap();

    fs::rename(lib.root.join("Old Name"), lib.root.join("New Name")).unwrap();
    lib.scan();
    let favourites = lib.library.favourites().unwrap();
    assert_eq!(favourites.entries[0].path, lib.root.join("New Name/01.wav"));
    assert_eq!(favourite_tracks(&lib), [Some(track)]);
}

#[test]
fn clearing_the_library_keeps_favourites() {
    let (mut lib, tracks) = library_with_tracks();
    lib.library.add_favourites(&[tracks[0].1]).unwrap();
    lib.library.clear_tracks().unwrap();
    assert_eq!(favourite_tracks(&lib), [None]);
}

#[test]
fn an_image_can_be_chosen_and_removed() {
    let lib = TestLibrary::new();
    assert_eq!(lib.library.favourites().unwrap().image, None);
    let image = CoverId("playlist-favourites-00000000000000aa.png".to_owned());
    lib.library.set_favourites_image(Some(&image)).unwrap();
    let other = CoverId("playlist-favourites-00000000000000bb.png".to_owned());
    lib.library.set_favourites_image(Some(&other)).unwrap();
    assert_eq!(lib.library.favourites().unwrap().image, Some(other));
    lib.library.set_favourites_image(None).unwrap();
    assert_eq!(lib.library.favourites().unwrap().image, None);
}
