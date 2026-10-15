//! Scans a music folder and prints a summary and one line per track.
//!
//! `cargo run -p mp-library --example scan -- <music folder> [data folder]`
//!
//! The database and extracted covers go to a temporary folder, or to `data folder` if
//! given so they can be inspected afterwards.

use std::error::Error;
use std::path::PathBuf;
use std::time::Instant;

use mp_core::util::format_duration;
use mp_library::{Library, ScanOptions};

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args_os().skip(1);
    let root = PathBuf::from(
        args.next()
            .ok_or("usage: scan <music folder> [data folder]")?,
    );
    let temp = tempfile::tempdir()?;
    let data = args
        .next()
        .map_or_else(|| temp.path().to_path_buf(), PathBuf::from);

    let mut library = Library::open(&data.join("library.db"))?;
    let options = ScanOptions {
        root: root.clone(),
        covers_dir: data.join("covers"),
        force: false,
    };

    let started = Instant::now();
    let summary = library.scan(&options, &|_| {})?;
    println!("Scan:   {summary:?} in {:.1?}", started.elapsed());
    let started = Instant::now();
    let rescan = library.scan(&options, &|_| {})?;
    println!("Rescan: {rescan:?} in {:.1?}", started.elapsed());
    println!("Data:   {}\n", data.display());

    for track in library.tracks()? {
        let number = match (track.disc_number, track.track_number) {
            (Some(disc), Some(number)) => format!("{disc}-{number:02}"),
            (None, Some(number)) => format!("{number:02}"),
            _ => "--".to_owned(),
        };
        let file = track.path.strip_prefix(&root).unwrap_or(&track.path);
        println!(
            "{} [{}] | {} ({}) | {number} {} | {} | {} | {} {} | {} ch | cover {} | {}",
            track.display_artist(),
            track.display_album_artist(),
            track.display_album(),
            track
                .year
                .map_or("----".to_owned(), |year| year.to_string()),
            track.title,
            track.genre,
            format_duration(track.duration()),
            track.format,
            track.quality_label(),
            track.channels.unwrap_or(0),
            track.cover.as_ref().map_or("-", |cover| cover.0.as_str()),
            file.display(),
        );
    }
    Ok(())
}
