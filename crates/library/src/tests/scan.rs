use std::fs;

use lofty::picture::PictureType;
use mp_core::{AudioFormat, CoverId};

use super::fixtures::{
    TagKind, TestLibrary, TestTags, jpeg, png, set_modified, set_mtime, wav_bytes, write_wav,
};
use crate::{LibraryError, ScanOptions, ScanProgress, ScanSummary};

fn album_tags(title: &'static str, track: &'static str) -> TestTags {
    TestTags {
        title,
        artist: "Harbor & Pine",
        album: "Coastal Drift",
        album_artist: "Harbor & Pine",
        date: "2019-05-01",
        track,
        disc: "1/2",
        ..TestTags::default()
    }
}

#[test]
fn first_scan_reads_tags_and_properties() {
    let mut lib = TestLibrary::new();
    let tagged = lib.path("Harbor & Pine/Coastal Drift/01 Tidewater.wav");
    write_wav(&tagged, &album_tags("Tidewater", "1/12"));
    let riff = lib.path("Singles/riff.wav");
    write_wav(
        &riff,
        &TestTags {
            kind: TagKind::RiffInfo,
            title: "Studio Bounce",
            artist: "Nova Static",
            track: "7",
            date: "2021",
            ..TestTags::default()
        },
    );
    let untagged = lib.path("Singles/untitled_demo.wav");
    write_wav(&untagged, &TestTags::default());
    fs::write(lib.path("Singles/notes.txt"), "not music").unwrap();
    write_wav(&lib.path(".hidden/secret.wav"), &TestTags::default());
    write_wav(&lib.path("Singles/.hidden.wav"), &TestTags::default());

    let progress = std::sync::Mutex::new(Vec::new());
    let options = lib.options();
    let summary = lib
        .library
        .scan(&options, &|update| progress.lock().unwrap().push(update))
        .unwrap();

    assert_eq!(
        summary,
        ScanSummary {
            added: 3,
            ..ScanSummary::default()
        }
    );
    assert_eq!(lib.library.track_count().unwrap(), 3);
    assert_eq!(
        progress.into_inner().unwrap().last(),
        Some(&ScanProgress {
            processed: 3,
            total: 3
        })
    );

    let track = lib.track_at(&tagged).unwrap();
    assert_eq!(track.title, "Tidewater");
    assert_eq!(track.artist, "Harbor & Pine");
    assert_eq!(track.album, "Coastal Drift");
    assert_eq!(track.album_artist, "Harbor & Pine");
    assert_eq!(track.year, Some(2019));
    assert_eq!(track.track_number, Some(1));
    assert_eq!(track.disc_number, Some(1));
    assert_eq!(track.format, AudioFormat::Wav);
    assert_eq!(track.sample_rate, Some(44_100));
    assert_eq!(track.bit_depth, Some(16));
    assert_eq!(track.channels, Some(2));
    assert_eq!(track.duration_ms, 100);
    assert_eq!(track.file_size, fs::metadata(&tagged).unwrap().len());
    assert!(track.added > 0);
    assert_eq!(lib.library.track(track.id).unwrap(), Some(track));

    let riff_track = lib.track_at(&riff).unwrap();
    assert_eq!(riff_track.title, "Studio Bounce");
    assert_eq!(riff_track.artist, "Nova Static");
    assert_eq!(riff_track.track_number, Some(7));
    assert_eq!(riff_track.year, Some(2021));

    let untagged_track = lib.track_at(&untagged).unwrap();
    assert_eq!(untagged_track.title, "untitled_demo");
    assert_eq!(untagged_track.artist, "");
    assert_eq!(untagged_track.cover, None);
}

#[test]
fn tracks_are_in_library_order() {
    let mut lib = TestLibrary::new();
    let tags = |title, artist, album, track, disc| TestTags {
        title,
        artist,
        album,
        track,
        disc,
        ..TestTags::default()
    };
    write_wav(
        &lib.path("a.wav"),
        &tags("Second", "beta", "Album", "2", "1"),
    );
    write_wav(
        &lib.path("b.wav"),
        &tags("Disc Two", "Beta", "album", "1", "2"),
    );
    write_wav(&lib.path("c.wav"), &tags("First", "Beta", "Album", "1", ""));
    write_wav(
        &lib.path("d.wav"),
        &tags("Unnumbered", "Beta", "Album", "", ""),
    );
    write_wav(
        &lib.path("e.wav"),
        &tags("Alpha Song", "Álpha", "Zed", "9", ""),
    );
    lib.scan();

    let titles: Vec<String> = lib
        .library
        .tracks()
        .unwrap()
        .into_iter()
        .map(|track| track.title)
        .collect();
    assert_eq!(
        titles,
        ["Alpha Song", "First", "Second", "Unnumbered", "Disc Two"]
    );
}

#[test]
fn unchanged_files_are_not_read_again() {
    let mut lib = TestLibrary::new();
    let path = lib.path("Album/01.wav");
    write_wav(&path, &album_tags("Original", "1"));
    write_wav(&lib.path("Album/02.wav"), &album_tags("Other", "2"));
    lib.scan();
    let before = lib.track_at(&path).unwrap();

    // Same size and modification time but unreadable content: a re-read would fail.
    let metadata = fs::metadata(&path).unwrap();
    fs::write(&path, vec![b'x'; metadata.len() as usize]).unwrap();
    set_modified(&path, metadata.modified().unwrap());

    let summary = lib.scan();
    assert_eq!(
        summary,
        ScanSummary {
            unchanged: 2,
            ..ScanSummary::default()
        }
    );
    assert!(!summary.changed());
    assert_eq!(lib.track_at(&path), Some(before));
}

#[test]
fn modified_files_are_updated_in_place() {
    let mut lib = TestLibrary::new();
    let path = lib.path("Album/01.wav");
    write_wav(&path, &album_tags("Before", "1"));
    set_mtime(&path, 1_000);
    lib.scan();
    let before = lib.track_at(&path).unwrap();

    write_wav(&path, &album_tags("After", "1"));
    set_mtime(&path, 2_000);
    let summary = lib.scan();

    assert_eq!(
        summary,
        ScanSummary {
            updated: 1,
            ..ScanSummary::default()
        }
    );
    let after = lib.track_at(&path).unwrap();
    assert_eq!(after.title, "After");
    assert_eq!(after.modified, 2_000);
    assert_eq!((after.id, after.added), (before.id, before.added));
}

#[test]
fn force_rereads_everything() {
    let mut lib = TestLibrary::new();
    write_wav(&lib.path("a.wav"), &TestTags::default());
    write_wav(&lib.path("b.wav"), &TestTags::default());
    lib.scan();

    let options = ScanOptions {
        force: true,
        ..lib.options()
    };
    let summary = lib.library.scan(&options, &|_| {}).unwrap();
    assert_eq!(
        summary,
        ScanSummary {
            updated: 2,
            ..ScanSummary::default()
        }
    );
}

#[test]
fn deleted_files_and_files_outside_the_root_are_removed() {
    let mut lib = TestLibrary::new();
    let deleted = lib.path("Old/gone.wav");
    let outside = lib.path("Elsewhere/kept-on-disk.wav");
    let inside = lib.path("Music/stays.wav");
    for path in [&deleted, &outside, &inside] {
        write_wav(path, &TestTags::default());
    }
    lib.scan();

    fs::remove_file(&deleted).unwrap();
    let summary = lib.scan();
    assert_eq!((summary.removed, summary.unchanged), (1, 2));
    assert_eq!(lib.track_at(&deleted), None);

    // Pointing the library at a subfolder drops everything outside it.
    lib.root = lib.root.join("Music");
    let summary = lib.scan();
    assert_eq!((summary.removed, summary.unchanged), (1, 1));
    assert_eq!(lib.track_at(&outside), None);
    assert!(lib.track_at(&inside).is_some());
}

#[test]
fn missing_music_folder_is_an_error_and_keeps_the_library() {
    let mut lib = TestLibrary::new();
    write_wav(&lib.path("a.wav"), &TestTags::default());
    lib.scan();

    let options = ScanOptions {
        root: lib.root.join("unplugged-drive"),
        ..lib.options()
    };
    let result = lib.library.scan(&options, &|_| {});
    assert!(matches!(result, Err(LibraryError::FolderUnavailable(_))));
    assert_eq!(lib.library.track_count().unwrap(), 1);

    // The watcher reporting the whole folder as gone, e.g. when its drive is unmounted.
    let unmounted = lib.root.with_file_name("unmounted");
    fs::rename(&lib.root, &unmounted).unwrap();
    let result = lib.library.scan_paths(&lib.options(), &[lib.root.clone()]);
    assert!(matches!(result, Err(LibraryError::FolderUnavailable(_))));
    assert_eq!(lib.library.track_count().unwrap(), 1);
}

#[test]
fn empty_music_folder_keeps_the_library_until_cleared() {
    let mut lib = TestLibrary::new();
    write_wav(&lib.path("Album/a.wav"), &TestTags::default());
    write_wav(&lib.path("Album/b.wav"), &TestTags::default());
    lib.scan();

    // An unmounted drive leaves only its empty mount point behind.
    fs::remove_dir_all(lib.path("Album")).unwrap();
    let result = lib.library.scan(&lib.options(), &|_| {});
    assert!(matches!(result, Err(LibraryError::FolderUnavailable(_))));
    assert_eq!(lib.library.track_count().unwrap(), 2);

    assert_eq!(lib.library.clear_tracks().unwrap(), 2);
    assert_eq!(lib.library.track_count().unwrap(), 0);
    // With nothing left to protect, scanning an empty folder is fine.
    assert_eq!(lib.scan(), ScanSummary::default());
}

#[test]
fn scan_paths_handles_files_new_folders_and_deleted_folders() {
    let mut lib = TestLibrary::new();
    let first = lib.path("A/01.wav");
    write_wav(&first, &TestTags::default());
    write_wav(&lib.path("A/02.wav"), &TestTags::default());
    let untouched = lib.path("C/stale.wav");
    write_wav(&untouched, &TestTags::default());
    lib.scan();

    // A new file in a known folder, and a changed one.
    let new_file = lib.path("A/03.wav");
    write_wav(&new_file, &TestTags::default());
    write_wav(&first, &album_tags("Retagged", "1"));
    set_mtime(&first, 5_000);
    // Files that change on disk but aren't passed in must be left alone.
    let unscanned = lib.path("C/new-but-not-scanned.wav");
    write_wav(&unscanned, &TestTags::default());
    fs::remove_file(&untouched).unwrap();

    let summary = lib.scan_paths(&[new_file, first.clone()]);
    assert_eq!(
        summary,
        ScanSummary {
            added: 1,
            updated: 1,
            ..ScanSummary::default()
        }
    );
    assert_eq!(lib.track_at(&first).unwrap().title, "Retagged");

    // A new folder.
    write_wav(&lib.path("B/CD 1/01.wav"), &TestTags::default());
    write_wav(&lib.path("B/CD 2/01.wav"), &TestTags::default());
    let new_dir = lib.root.join("B");
    let summary = lib.scan_paths(std::slice::from_ref(&new_dir));
    assert_eq!(summary.added, 2);

    // A deleted folder, plus a path outside the music folder that must be ignored.
    let deleted_dir = lib.root.join("A");
    fs::remove_dir_all(&deleted_dir).unwrap();
    let summary = lib.scan_paths(&[deleted_dir, "/definitely/not/music".into()]);
    assert_eq!(
        summary,
        ScanSummary {
            removed: 3,
            ..ScanSummary::default()
        }
    );
    assert_eq!(lib.track_at(&first), None);
    assert!(lib.track_at(&untouched).is_some());
    assert_eq!(lib.track_at(&unscanned), None);
    assert_eq!(lib.library.track_count().unwrap(), 3);

    // A deleted single file.
    let summary = lib.scan_paths(std::slice::from_ref(&untouched));
    assert_eq!(summary.removed, 1);
}

#[test]
fn scan_paths_does_not_confuse_folders_with_similar_names() {
    let mut lib = TestLibrary::new();
    write_wav(&lib.path("Album/01.wav"), &TestTags::default());
    write_wav(&lib.path("Album 2/01.wav"), &TestTags::default());
    write_wav(&lib.path("Albums/01.wav"), &TestTags::default());
    lib.scan();

    fs::remove_dir_all(lib.root.join("Album")).unwrap();
    let summary = lib.scan_paths(&[lib.root.join("Album")]);
    assert_eq!(summary.removed, 1);
    assert_eq!(lib.library.track_count().unwrap(), 2);
}

#[test]
fn scan_paths_picks_up_a_folder_image_in_the_music_folder_itself() {
    let mut lib = TestLibrary::new();
    let in_root = lib.path("01.wav");
    let in_album = lib.path("Album/01.wav");
    write_wav(&in_root, &TestTags::default());
    write_wav(&in_album, &TestTags::default());
    lib.scan();

    let image = lib.root.join("cover.jpg");
    fs::write(&image, jpeg(1)).unwrap();
    assert_eq!(lib.scan_paths(&[image]).updated, 1);
    assert!(lib.track_at(&in_root).unwrap().cover.is_some());
    assert_eq!(lib.track_at(&in_album).unwrap().cover, None);
}

#[test]
fn a_file_moved_over_another_replaces_its_track() {
    let mut lib = TestLibrary::new();
    let moved = lib.path("A/01.wav");
    let replaced = lib.path("B/01.wav");
    write_wav(&moved, &album_tags("Moved", "1"));
    write_wav(&replaced, &album_tags("Replaced", "1"));
    lib.scan();

    // As the watcher reports it: both sides of the rename.
    fs::rename(&moved, &replaced).unwrap();
    let summary = lib.scan_paths(&[moved, replaced.clone()]);
    assert_eq!((summary.updated, summary.removed), (1, 1));
    assert_eq!(lib.library.track_count().unwrap(), 1);
    assert_eq!(lib.track_at(&replaced).unwrap().title, "Moved");
}

#[test]
fn unicode_file_names_round_trip() {
    let mut lib = TestLibrary::new();
    let path = lib.path("Kōji Arai/Night Transit/1-01 Shinjuku 2 a.m..wav");
    write_wav(&path, &TestTags::default());
    assert_eq!(lib.scan().added, 1);

    let track = lib.track_at(&path).unwrap();
    assert_eq!(track.title, "1-01 Shinjuku 2 a.m.");
    assert_eq!(lib.scan().unchanged, 1);
}

#[cfg(unix)]
#[test]
fn non_utf8_file_names_round_trip() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let mut lib = TestLibrary::new();
    let path = lib.root.join(OsStr::from_bytes(b"caf\xe9.wav"));
    write_wav(&path, &TestTags::default());
    write_wav(&lib.path("other.wav"), &TestTags::default());
    assert_eq!(lib.scan().added, 2);

    let track = lib.track_at(&path).unwrap();
    assert_eq!(track.path, path);
    assert_eq!(track.title, "caf\u{FFFD}");
    assert_eq!(lib.scan().unchanged, 2);

    fs::remove_file(&path).unwrap();
    assert_eq!(lib.scan_paths(&[path]).removed, 1);
}

#[test]
fn embedded_covers_are_extracted_once_and_restored_when_missing() {
    let mut lib = TestLibrary::new();
    let front = png(1);
    let with_front = TestTags {
        pictures: vec![
            (PictureType::Other, png(9)),
            (PictureType::CoverFront, front.clone()),
        ],
        ..TestTags::default()
    };
    let first = lib.path("Album/01.wav");
    let second = lib.path("Album/02.wav");
    write_wav(&first, &with_front);
    write_wav(&second, &with_front);
    let only_other = lib.path("Other/01.wav");
    write_wav(
        &only_other,
        &TestTags {
            pictures: vec![(PictureType::Other, png(2))],
            ..TestTags::default()
        },
    );
    lib.scan();

    let expected = CoverId(format!("{:016x}.png", mp_core::util::stable_hash(&front)));
    assert_eq!(lib.track_at(&first).unwrap().cover, Some(expected.clone()));
    assert_eq!(lib.track_at(&second).unwrap().cover, Some(expected.clone()));
    let other_cover = lib.track_at(&only_other).unwrap().cover.unwrap();
    assert_ne!(other_cover, expected);
    // The shared front cover is written once.
    assert_eq!(lib.cover_files().len(), 2);
    assert_eq!(fs::read(lib.covers.join(&expected.0)).unwrap(), front);

    fs::remove_file(lib.covers.join(&expected.0)).unwrap();
    let summary = lib.scan();
    assert_eq!((summary.updated, summary.unchanged), (2, 1));
    assert_eq!(fs::read(lib.covers.join(&expected.0)).unwrap(), front);
}

#[test]
fn folder_images_are_used_when_nothing_is_embedded() {
    let mut lib = TestLibrary::new();
    let folder_image = jpeg(3);
    fs::write(lib.path("Album/Folder.JPG"), &folder_image).unwrap();
    fs::write(lib.path("Album/back.jpg"), jpeg(4)).unwrap();
    let plain = lib.path("Album/01.wav");
    write_wav(&plain, &TestTags::default());
    let embedded = lib.path("Album/02.wav");
    write_wav(
        &embedded,
        &TestTags {
            pictures: vec![(PictureType::CoverFront, png(5))],
            ..TestTags::default()
        },
    );
    let broken_art = lib.path("Album/03.wav");
    write_wav(
        &broken_art,
        &TestTags {
            pictures: vec![(PictureType::CoverFront, b"not an image".to_vec())],
            ..TestTags::default()
        },
    );
    let no_art = lib.path("Bare/01.wav");
    write_wav(&no_art, &TestTags::default());
    lib.scan();

    let expected = CoverId(format!(
        "{:016x}.jpg",
        mp_core::util::stable_hash(&folder_image)
    ));
    assert_eq!(lib.track_at(&plain).unwrap().cover, Some(expected.clone()));
    assert_eq!(lib.track_at(&broken_art).unwrap().cover, Some(expected));
    assert!(
        lib.track_at(&embedded)
            .unwrap()
            .cover
            .unwrap()
            .0
            .ends_with(".png")
    );
    assert_eq!(lib.track_at(&no_art).unwrap().cover, None);
}

#[test]
fn unreadable_files_fail_without_stopping_the_scan() {
    let mut lib = TestLibrary::new();
    fs::write(lib.path("broken.flac"), "this is not a flac file").unwrap();
    write_wav(&lib.path("fine.wav"), &TestTags::default());

    let summary = lib.scan();
    assert_eq!(
        summary,
        ScanSummary {
            added: 1,
            failed: 1,
            ..ScanSummary::default()
        }
    );
    // Failed files aren't stored, so they are tried again next time.
    assert_eq!(lib.scan().failed, 1);
}

#[test]
fn unknown_containers_are_added_with_their_file_name() {
    let mut lib = TestLibrary::new();
    let mka = lib.path("Live Set.mka");
    // An EBML (Matroska) header, which lofty doesn't read.
    fs::write(&mka, [0x1A, 0x45, 0xDF, 0xA3, 0x9F, 0x42, 0x86, 0x81, 0x01]).unwrap();
    lib.scan();

    let track = lib.track_at(&mka).unwrap();
    assert_eq!(track.title, "Live Set");
    assert_eq!(track.format, AudioFormat::Unknown);
    assert_eq!(track.duration_ms, 0);
}

/// Different spellings of the music folder store a modified file under the same key,
/// so its track isn't duplicated.
#[test]
fn other_spellings_of_the_music_folder_do_not_duplicate_tracks() {
    let mut lib = TestLibrary::new();
    let path = lib.path("Album/01.wav");
    write_wav(&path, &album_tags("Before", "1"));
    set_mtime(&path, 1_000);
    lib.scan();

    write_wav(&path, &album_tags("After", "1"));
    set_mtime(&path, 2_000);
    let options = ScanOptions {
        root: lib.root.join("."),
        ..lib.options()
    };
    let summary = lib.library.scan(&options, &|_| {}).unwrap();
    assert_eq!(summary.updated, 1);

    let spelled_oddly = lib.root.join(".//Album/./01.wav");
    let summary = lib.library.scan_paths(&options, &[spelled_oddly]).unwrap();
    assert_eq!(summary.unchanged, 1);
    assert_eq!(lib.library.track_count().unwrap(), 1);
    assert_eq!(lib.track_at(&path).unwrap().title, "After");
}

#[test]
fn scan_paths_ignores_paths_that_climb_out_of_the_music_folder() {
    let mut lib = TestLibrary::new();
    let outside = lib.root.with_file_name("outside");
    fs::create_dir(&outside).unwrap();
    write_wav(&outside.join("x.wav"), &TestTags::default());

    let summary = lib.scan_paths(&[
        lib.root.join("../outside"),
        lib.root.join("../outside/x.wav"),
    ]);
    assert_eq!(summary, ScanSummary::default());
    assert_eq!(lib.library.track_count().unwrap(), 0);
}

/// Moving or renaming a folder keeps its tracks rather than deleting and adding them
/// again, which would lose their ids, "added" times and the playlist entries pointing
/// at them.
#[test]
fn moved_folders_keep_their_tracks_and_playlist_entries() {
    let mut lib = TestLibrary::new();
    let before = [lib.path("Old Name/01.wav"), lib.path("Old Name/02.wav")];
    write_wav(&before[0], &album_tags("One", "1"));
    write_wav(&before[1], &album_tags("Two", "2"));
    lib.scan();
    let originals: Vec<_> = before
        .iter()
        .map(|path| lib.track_at(path).unwrap())
        .collect();
    let playlist = lib.library.create_playlist("Mix").unwrap();
    let ids: Vec<_> = originals.iter().map(|track| track.id).collect();
    lib.library.add_to_playlist(playlist, &ids).unwrap();

    fs::rename(lib.root.join("Old Name"), lib.root.join("New Name")).unwrap();
    let summary = lib.scan();
    assert_eq!(
        summary,
        ScanSummary {
            updated: 2,
            ..ScanSummary::default()
        }
    );
    let after = [
        lib.root.join("New Name/01.wav"),
        lib.root.join("New Name/02.wav"),
    ];
    for (original, path) in originals.iter().zip(&after) {
        let moved = lib.track_at(path).unwrap();
        assert_eq!((moved.id, moved.added), (original.id, original.added));
        assert_eq!(moved.title, original.title);
    }
    let entries = lib.library.playlist(playlist).unwrap().unwrap().entries;
    let paths: Vec<_> = entries.iter().map(|entry| entry.path.clone()).collect();
    assert_eq!(paths, after);
    assert!(entries.iter().all(|entry| entry.track.is_some()));

    // The same through the watcher's view: both sides of the rename.
    fs::rename(lib.root.join("New Name"), lib.root.join("Newer")).unwrap();
    let summary = lib.scan_paths(&[lib.root.join("New Name"), lib.root.join("Newer")]);
    assert_eq!((summary.updated, summary.removed, summary.added), (2, 0, 0));
    let moved = lib.track_at(&lib.root.join("Newer/01.wav")).unwrap();
    assert_eq!(moved.id, originals[0].id);
    assert_eq!(
        lib.library
            .playlist(playlist)
            .unwrap()
            .unwrap()
            .available_tracks()
            .count(),
        2
    );
}

#[test]
fn ambiguous_moves_are_treated_as_removal_and_addition() {
    let mut lib = TestLibrary::new();
    let twins = [lib.path("A/01.wav"), lib.path("B/01.wav")];
    for path in &twins {
        write_wav(path, &TestTags::default());
        set_mtime(path, 1_000);
    }
    lib.scan();

    fs::create_dir(lib.root.join("C")).unwrap();
    fs::rename(&twins[0], lib.root.join("C/01.wav")).unwrap();
    fs::remove_file(&twins[1]).unwrap();
    let summary = lib.scan();
    assert_eq!((summary.added, summary.removed, summary.updated), (1, 2, 0));
}

#[test]
fn large_scans_are_saved_in_batches_with_steady_progress() {
    let mut lib = TestLibrary::new();
    let count = 2 * crate::scanner::WRITE_BATCH + 7;
    let tiny_wav = wav_bytes(8_000, 1, 16, 8);
    for index in 0..count {
        fs::write(lib.path(&format!("{}/{index}.wav", index % 50)), &tiny_wav).unwrap();
    }

    let progress = std::sync::Mutex::new(Vec::new());
    let options = lib.options();
    let summary = lib
        .library
        .scan(&options, &|update| {
            progress.lock().unwrap().push(update.processed)
        })
        .unwrap();
    assert_eq!(summary.added, count);
    assert_eq!(lib.library.track_count().unwrap(), count);
    let progress = progress.into_inner().unwrap();
    assert_eq!(progress, (0..=count).collect::<Vec<_>>());
}

#[test]
fn cancelled_scans_stop_and_keep_the_batches_saved() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let mut lib = TestLibrary::new();
    let count = crate::scanner::WRITE_BATCH + 7;
    let tiny_wav = wav_bytes(8_000, 1, 16, 8);
    for index in 0..count {
        fs::write(lib.path(&format!("{}/{index}.wav", index % 50)), &tiny_wav).unwrap();
    }
    let options = lib.options();

    let result = lib.library.scan_cancellable(&options, &|_| {}, &|| true);
    assert!(matches!(result, Err(LibraryError::Cancelled)));
    assert_eq!(lib.library.track_count().unwrap(), 0);

    // Cancelled while the second batch is being read.
    let cancel = AtomicBool::new(false);
    let result = lib.library.scan_cancellable(
        &options,
        &|progress| {
            if progress.processed > crate::scanner::WRITE_BATCH {
                cancel.store(true, Ordering::Relaxed);
            }
        },
        &|| cancel.load(Ordering::Relaxed),
    );
    assert!(matches!(result, Err(LibraryError::Cancelled)));
    assert_eq!(
        lib.library.track_count().unwrap(),
        crate::scanner::WRITE_BATCH
    );
    assert_eq!(lib.scan().added, 7);
}

#[cfg(target_os = "linux")]
#[test]
fn names_in_different_unicode_normalisation_forms_stay_distinct() {
    let mut lib = TestLibrary::new();
    let composed = lib.path("Caf\u{e9}.wav");
    let decomposed = lib.path("Cafe\u{301}.wav");
    write_wav(&composed, &TestTags::default());
    write_wav(&decomposed, &TestTags::default());
    assert_eq!(lib.scan().added, 2);
    assert_eq!(lib.scan().unchanged, 2);

    fs::remove_file(&decomposed).unwrap();
    assert_eq!(lib.scan_paths(&[decomposed]).removed, 1);
    assert!(lib.track_at(&composed).is_some());
}

#[cfg(unix)]
#[test]
fn symlink_loops_are_skipped() {
    let mut lib = TestLibrary::new();
    write_wav(&lib.path("Album/01.wav"), &TestTags::default());
    std::os::unix::fs::symlink(&lib.root, lib.root.join("Album/loop")).unwrap();

    assert_eq!(lib.scan().added, 1);
    assert_eq!(lib.scan().unchanged, 1);
}

#[cfg(unix)]
#[test]
fn unreadable_folders_keep_their_tracks() {
    use std::os::unix::fs::PermissionsExt;

    let mut lib = TestLibrary::new();
    let locked = lib.root.join("Locked");
    write_wav(&lib.path("Locked/01.wav"), &TestTags::default());
    write_wav(&lib.path("Open/01.wav"), &TestTags::default());
    lib.scan();

    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    // Privileged users can read the folder anyway, which makes this test meaningless.
    let readable = fs::read_dir(&locked).is_ok();
    let summary = lib.scan();
    let removed_by_paths = lib.scan_paths(std::slice::from_ref(&locked)).removed;
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    if !readable {
        assert_eq!((summary.removed, removed_by_paths), (0, 0));
        assert_eq!(lib.library.track_count().unwrap(), 2);
    }
}

/// A symlink to a folder on a drive that isn't mounted keeps its tracks, like a folder
/// that can't be read, also when one of its files is re-checked.
#[cfg(unix)]
#[test]
fn tracks_behind_a_dangling_symlink_are_kept() {
    let mut lib = TestLibrary::new();
    let drive = lib.root.with_file_name("drive");
    fs::create_dir(&drive).unwrap();
    write_wav(&drive.join("01.wav"), &TestTags::default());
    std::os::unix::fs::symlink(&drive, lib.root.join("NAS")).unwrap();
    write_wav(&lib.path("Local/01.wav"), &TestTags::default());
    assert_eq!(lib.scan().added, 2);

    fs::rename(&drive, drive.with_file_name("unmounted")).unwrap();
    let behind_link = lib.root.join("NAS/01.wav");
    let unchanged = ScanSummary {
        unchanged: 1,
        ..ScanSummary::default()
    };
    assert_eq!(lib.scan(), unchanged);
    assert_eq!(
        lib.scan_paths(&[lib.root.join("NAS")]),
        ScanSummary::default()
    );
    assert_eq!(
        lib.scan_paths(std::slice::from_ref(&behind_link)),
        ScanSummary::default()
    );
    assert!(lib.track_at(&behind_link).is_some());

    // Removing the link itself does remove its tracks.
    fs::remove_file(lib.root.join("NAS")).unwrap();
    assert_eq!(lib.scan().removed, 1);
}
