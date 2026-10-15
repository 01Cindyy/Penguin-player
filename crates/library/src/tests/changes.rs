//! Noticing changes that a file's size and its modification time in whole seconds don't
//! show, and keeping the covers cache in step with the library.

use std::fs;
use std::time::Duration;

use lofty::picture::PictureType;
use mp_core::CoverId;
use mp_core::util::stable_hash;

use super::fixtures::{
    TestLibrary, TestTags, jpeg, png, set_mtime, set_mtime_precisely, write_wav,
};
use crate::ScanSummary;

fn cover_id(image: &[u8], extension: &str) -> CoverId {
    CoverId(format!("{:016x}.{extension}", stable_hash(image)))
}

fn with_picture(image: Vec<u8>) -> TestTags {
    TestTags {
        pictures: vec![(PictureType::CoverFront, image)],
        ..TestTags::default()
    }
}

fn titled(title: &'static str) -> TestTags {
    TestTags {
        title,
        ..TestTags::default()
    }
}

/// Times are compared to the nanosecond, so a tag editor rewriting a file in place
/// (same size, thanks to tag padding) within the same second is noticed.
#[test]
fn rewrites_within_the_same_second_are_noticed() {
    let mut lib = TestLibrary::new();
    let path = lib.path("Album/01.wav");
    write_wav(&path, &titled("Before"));
    set_mtime_precisely(&path, Duration::new(1_000, 100_000_000));
    lib.scan();
    let size = fs::metadata(&path).unwrap().len();

    write_wav(&path, &titled("Afters"));
    assert_eq!(fs::metadata(&path).unwrap().len(), size);
    set_mtime_precisely(&path, Duration::new(1_000, 900_000_000));

    assert_eq!(
        lib.scan_paths(std::slice::from_ref(&path)),
        ScanSummary {
            updated: 1,
            ..ScanSummary::default()
        }
    );
    assert_eq!(lib.track_at(&path).unwrap().title, "Afters");
}

/// Rows saved before sub-second times were stored are read once more to fill them in.
#[test]
fn tracks_saved_without_sub_second_times_are_read_once_more() {
    let mut lib = TestLibrary::new();
    write_wav(&lib.path("a.wav"), &TestTags::default());
    lib.scan();
    lib.library
        .conn
        .execute(
            "UPDATE tracks SET modified_nanos = NULL, folder_images = NULL",
            [],
        )
        .unwrap();

    let updated = ScanSummary {
        updated: 1,
        ..ScanSummary::default()
    };
    assert_eq!(lib.scan(), updated);
    let unchanged = ScanSummary {
        unchanged: 1,
        ..ScanSummary::default()
    };
    assert_eq!(lib.scan(), unchanged);
}

/// A folder image added (or replaced, or removed) after its tracks were scanned updates
/// their covers, although the audio files themselves didn't change.
#[test]
fn folder_images_added_changed_or_removed_later_update_covers() {
    let mut lib = TestLibrary::new();
    let plain = lib.path("Album/01.wav");
    write_wav(&plain, &TestTags::default());
    let embedded = lib.path("Album/02.wav");
    write_wav(&embedded, &with_picture(png(1)));
    lib.scan();
    assert_eq!(lib.track_at(&plain).unwrap().cover, None);
    let embedded_cover = Some(cover_id(&png(1), "png"));
    assert_eq!(lib.track_at(&embedded).unwrap().cover, embedded_cover);

    let image = lib.root.join("Album/cover.jpg");
    fs::write(&image, jpeg(1)).unwrap();
    let one_updated = ScanSummary {
        updated: 1,
        unchanged: 1,
        ..ScanSummary::default()
    };
    assert_eq!(lib.scan(), one_updated);
    assert_eq!(
        lib.track_at(&plain).unwrap().cover,
        Some(cover_id(&jpeg(1), "jpg"))
    );

    // Same size: only the modification time tells the new image apart.
    fs::write(&image, jpeg(2)).unwrap();
    set_mtime(&image, 5_000);
    assert_eq!(lib.scan(), one_updated);
    assert_eq!(
        lib.track_at(&plain).unwrap().cover,
        Some(cover_id(&jpeg(2), "jpg"))
    );

    fs::remove_file(&image).unwrap();
    assert_eq!(lib.scan(), one_updated);
    assert_eq!(lib.track_at(&plain).unwrap().cover, None);
    // An embedded cover wins over folder images, so its track was never read again.
    assert_eq!(lib.track_at(&embedded).unwrap().cover, embedded_cover);
}

#[test]
fn scanning_a_folder_image_rechecks_its_folder() {
    let mut lib = TestLibrary::new();
    let inside = lib.path("Album/01.wav");
    let elsewhere = lib.path("Other/01.wav");
    write_wav(&inside, &TestTags::default());
    write_wav(&elsewhere, &TestTags::default());
    lib.scan();

    let image = lib.root.join("Album/Folder.png");
    fs::write(&image, png(3)).unwrap();
    assert_eq!(
        lib.scan_paths(&[image]),
        ScanSummary {
            updated: 1,
            ..ScanSummary::default()
        }
    );
    assert_eq!(
        lib.track_at(&inside).unwrap().cover,
        Some(cover_id(&png(3), "png"))
    );
    assert_eq!(lib.track_at(&elsewhere).unwrap().cover, None);
}

/// Covers replaced by new ones or left behind by removed tracks don't stay in the
/// cache forever.
#[test]
fn full_scans_delete_covers_no_track_uses() {
    let mut lib = TestLibrary::new();
    let kept = lib.path("Kept/01.wav");
    let retagged = lib.path("Retagged/01.wav");
    let removed = lib.path("Removed/01.wav");
    write_wav(&kept, &with_picture(png(1)));
    write_wav(&retagged, &with_picture(png(2)));
    write_wav(&removed, &with_picture(png(3)));
    lib.scan();
    assert_eq!(lib.cover_files().len(), 3);
    // Only files named like the cache's own are ever deleted, and temporary files only
    // once they are too old to belong to a write that is still going on.
    let fresh_temporary = "0123456789abcdef.123-4.tmp";
    let stale_temporary = "0123456789abcdef.123-5.tmp";
    for name in ["notes.txt", "photo.jpg", fresh_temporary, stale_temporary] {
        fs::write(lib.covers.join(name), b"x").unwrap();
    }
    set_mtime(&lib.covers.join(stale_temporary), 1_000);

    write_wav(&retagged, &with_picture(png(4)));
    set_mtime(&retagged, 5_000);
    fs::remove_file(&removed).unwrap();
    // A partial scan doesn't know every track's cover, so it deletes nothing.
    lib.scan_paths(&[retagged.clone(), removed]);
    assert_eq!(lib.cover_files().len(), 8);

    lib.scan();
    let mut expected = vec![
        cover_id(&png(1), "png").0,
        cover_id(&png(4), "png").0,
        fresh_temporary.to_owned(),
        "notes.txt".to_owned(),
        "photo.jpg".to_owned(),
    ];
    expected.sort();
    assert_eq!(lib.cover_files(), expected);
    assert_eq!(
        lib.track_at(&retagged).unwrap().cover,
        Some(cover_id(&png(4), "png"))
    );
}

/// A moved file that fails to be read again still counts as updated, so a scan that
/// moved tracks reports the change and the UI gets the new paths.
#[test]
fn moved_files_that_can_no_longer_be_read_still_count_as_updated() {
    let mut lib = TestLibrary::new();
    let path = lib.path("Old/01.wav");
    write_wav(&path, &titled("Kept Title"));
    set_mtime(&path, 1_000);
    lib.scan();
    let original = lib.track_at(&path).unwrap();

    // Same size and time, but content a re-read can't parse.
    let size = fs::metadata(&path).unwrap().len();
    fs::write(&path, vec![b'x'; size as usize]).unwrap();
    set_mtime(&path, 1_000);
    fs::rename(lib.root.join("Old"), lib.root.join("New")).unwrap();

    let summary = lib.scan();
    assert_eq!(
        summary,
        ScanSummary {
            updated: 1,
            ..ScanSummary::default()
        }
    );
    assert!(summary.changed());
    let moved = lib.track_at(&lib.root.join("New/01.wav")).unwrap();
    assert_eq!(
        (moved.id, moved.title.as_str()),
        (original.id, "Kept Title")
    );
}
