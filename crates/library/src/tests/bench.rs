//! Rough timings for a large library. Not run by default:
//! `cargo test -p mp-library --release -- --ignored --nocapture large_library`

use std::fs::{self, File};
use std::time::{Duration, Instant, UNIX_EPOCH};

use mp_core::{PlaylistItemId, TrackId};

use super::fixtures::{TestLibrary, wav_bytes};
use crate::tracks;

const ALBUMS: usize = 2_000;
const TRACKS_PER_ALBUM: usize = 10;
const ARTISTS: usize = 300;
const PLAYLISTS: usize = 100;
const PLAYLIST_LENGTH: usize = 200;
const LONG_PLAYLIST_LENGTH: usize = 10_000;
const DRAGGED_ROWS: usize = 500;

#[test]
#[ignore = "benchmark: run explicitly with --ignored"]
fn large_library_timings() {
    let mut lib = TestLibrary::new();
    let total = ALBUMS * TRACKS_PER_ALBUM;
    let started = Instant::now();
    // Tiny but real WAV files, so a scan really reads them. Sizes differ like real songs.
    for album in 0..ALBUMS {
        let dir = lib
            .root
            .join(format!("Artist {}/Album {album}", album % ARTISTS));
        fs::create_dir_all(&dir).unwrap();
        for number in 1..=TRACKS_PER_ALBUM {
            let path = dir.join(format!("{number:02} Song {number}.wav"));
            let frames = (album % 97 * TRACKS_PER_ALBUM + number) as u32;
            fs::write(&path, wav_bytes(8_000, 1, 16, frames)).unwrap();
            File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(UNIX_EPOCH + Duration::from_secs(1_700_000_000))
                .unwrap();
        }
    }
    println!("setup: {total} files in {:?}", started.elapsed());

    let started = Instant::now();
    let summary = lib.scan();
    println!("first scan: {summary:?} in {:?}", started.elapsed());
    assert_eq!(summary.added, total);

    let started = Instant::now();
    let all = lib.library.tracks().unwrap();
    println!("tracks(): {} tracks in {:?}", all.len(), started.elapsed());
    assert_eq!(all.len(), total);

    let started = Instant::now();
    let stored = tracks::all_files(&lib.library.conn).unwrap();
    println!(
        "stored file state (DB side of the diff): {} rows in {:?}",
        stored.len(),
        started.elapsed()
    );

    let started = Instant::now();
    let summary = lib.scan();
    println!(
        "unchanged rescan (walk + diff): {summary:?} in {:?}",
        started.elapsed()
    );
    assert_eq!(summary.unchanged, total);
    assert!(!summary.changed());

    let ids: Vec<TrackId> = all.iter().map(|track| track.id).collect();
    for index in 0..PLAYLISTS {
        let playlist = lib
            .library
            .create_playlist(&format!("Playlist {index}"))
            .unwrap();
        let start = index * PLAYLIST_LENGTH % (total - PLAYLIST_LENGTH);
        lib.library
            .add_to_playlist(playlist, &ids[start..start + PLAYLIST_LENGTH])
            .unwrap();
    }
    let started = Instant::now();
    let playlists = lib.library.playlists().unwrap();
    println!(
        "playlists(): {} playlists of {PLAYLIST_LENGTH} in {:?}",
        playlists.len(),
        started.elapsed()
    );

    // Dragging 500 rows to the end of a 10,000-song playlist.
    let long = lib.library.create_playlist("Long").unwrap();
    lib.library
        .add_to_playlist(long, &ids[..LONG_PLAYLIST_LENGTH])
        .unwrap();
    let items: Vec<PlaylistItemId> = lib
        .library
        .playlist(long)
        .unwrap()
        .unwrap()
        .entries
        .iter()
        .map(|entry| entry.item)
        .collect();
    let (dragged, rest): (Vec<_>, Vec<_>) = items
        .iter()
        .enumerate()
        .partition(|(index, _)| index % (LONG_PLAYLIST_LENGTH / DRAGGED_ROWS) == 0);
    let order: Vec<PlaylistItemId> = rest
        .into_iter()
        .chain(dragged)
        .map(|(_, &item)| item)
        .collect();
    let started = Instant::now();
    lib.library.reorder_playlist(long, &order).unwrap();
    println!(
        "reorder_playlist(): {DRAGGED_ROWS} of {LONG_PLAYLIST_LENGTH} entries moved in {:?}",
        started.elapsed()
    );

    // Renaming an artist folder moves every track inside it.
    let moved = all
        .iter()
        .filter(|track| track.path.starts_with(lib.root.join("Artist 0")))
        .count();
    fs::rename(lib.root.join("Artist 0"), lib.root.join("Renamed Artist")).unwrap();
    let started = Instant::now();
    let summary = lib.scan();
    println!(
        "rescan after moving {moved} tracks: {summary:?} in {:?}",
        started.elapsed()
    );
    assert_eq!(
        (summary.updated, summary.added, summary.removed),
        (moved, 0, 0)
    );
}
