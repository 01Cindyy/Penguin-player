//! Decodes every supported audio file under a folder as fast as possible, without playing
//! anything, and prints what was found. Handy for checking codec support and durations.
//!
//! Usage: `cargo run -p mp-audio --release --example decode_check -- <folder>`

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use mp_audio::{DecodeError, TrackDecoder};

fn main() -> ExitCode {
    let Some(folder) = std::env::args_os().nth(1).map(PathBuf::from) else {
        eprintln!("usage: decode_check <folder>");
        return ExitCode::FAILURE;
    };

    let mut files = Vec::new();
    collect_audio_files(&folder, &mut files);
    files.sort();

    let mut failures = 0;
    let started = Instant::now();
    for path in &files {
        let name = path.strip_prefix(&folder).unwrap_or(path).display();
        match decode(path) {
            Ok(report) => println!("ok    {name}\n      {report}"),
            Err(error) => {
                failures += 1;
                println!("FAIL  {name}\n      {error}");
            }
        }
    }
    println!(
        "\n{} files, {failures} failed, {:.2?} total",
        files.len(),
        started.elapsed()
    );
    if failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn decode(path: &Path) -> Result<String, DecodeError> {
    let started = Instant::now();
    let mut decoder = TrackDecoder::open(path)?;
    let info = decoder.info().clone();
    let mut samples = 0_u64;
    while let Some(chunk) = decoder.next_chunk()? {
        samples += chunk.len() as u64;
    }
    let frames = samples / u64::from(info.channels);
    let decoded = Duration::from_secs_f64(frames as f64 / f64::from(info.sample_rate));
    Ok(format!(
        "{codec}, {rate} Hz, {channels} ch, {frames} frames = {decoded:.4}s decoded \
         (reported {reported}) in {elapsed:.1?}",
        codec = info.codec,
        rate = info.sample_rate,
        channels = info.channels,
        decoded = decoded.as_secs_f64(),
        reported = info
            .duration
            .map_or("unknown".to_owned(), |d| format!("{:.4}s", d.as_secs_f64())),
        elapsed = started.elapsed(),
    ))
}

fn collect_audio_files(folder: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_audio_files(&path, files);
        } else if path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(mp_core::format::is_supported_extension)
        {
            files.push(path);
        }
    }
}
