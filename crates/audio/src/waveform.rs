//! The loudness of a whole song from start to end, for drawing it as a waveform.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::decoder::{DecodeError, TrackDecoder};

/// How many values a waveform has, however long the song. More than any seek bar has
/// bars, so that it can be drawn at any width.
pub const WAVEFORM_POINTS: usize = 512;
/// Loudness is first measured over windows of a twentieth of a second.
const WINDOWS_PER_SECOND: u32 = 20;

/// Decodes the file at `path` and measures its loudness along its length:
/// [`WAVEFORM_POINTS`] values, with 255 at the song's loudest point. Returns `None` if
/// `cancelled` got set meanwhile. Takes a moment, so call it off the main thread.
pub fn analyse_waveform(
    path: &Path,
    cancelled: &AtomicBool,
) -> Result<Option<Vec<u8>>, DecodeError> {
    let mut decoder = TrackDecoder::open(path)?;
    let info = decoder.info();
    let window =
        (info.sample_rate / WINDOWS_PER_SECOND).max(1) as usize * usize::from(info.channels).max(1);
    let mut windows = Vec::new();
    let (mut energy, mut samples) = (0.0_f32, 0_usize);
    while let Some(chunk) = decoder.next_chunk()? {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        for sample in chunk {
            energy += sample * sample;
            samples += 1;
            if samples == window {
                windows.push(energy / samples as f32);
                (energy, samples) = (0.0, 0);
            }
        }
    }
    if samples > 0 {
        windows.push(energy / samples as f32);
    }
    Ok(Some(points(&windows)))
}

/// Spreads the mean energy of `windows` over [`WAVEFORM_POINTS`] values, as their loudness
/// (root mean square) relative to the loudest. Silence stays all zeros.
fn points(windows: &[f32]) -> Vec<u8> {
    if windows.is_empty() {
        return vec![0; WAVEFORM_POINTS];
    }
    let loudness: Vec<f32> = (0..WAVEFORM_POINTS)
        .map(|point| {
            let start = point * windows.len() / WAVEFORM_POINTS;
            let end = ((point + 1) * windows.len() / WAVEFORM_POINTS).max(start + 1);
            let part = &windows[start..end];
            (part.iter().sum::<f32>() / part.len() as f32).sqrt()
        })
        .collect();
    let loudest = loudness.iter().copied().fold(0.0, f32::max);
    if loudest <= 0.0 {
        return vec![0; WAVEFORM_POINTS];
    }
    loudness
        .iter()
        .map(|level| (level / loudest * 255.0).round() as u8)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_files::write_wav;

    /// A tone at the given amplitude for the first half of the song, and another for the
    /// second half.
    fn two_part_song(path: &Path, first: f64, second: f64) {
        let rate = 8_000;
        let frames = rate * 4;
        write_wav(path, rate, 1, frames, |frame| {
            let amplitude = if frame < frames / 2 { first } else { second };
            let phase = f64::from(frame) * 440.0 / f64::from(rate) * std::f64::consts::TAU;
            (phase.sin() * amplitude * f64::from(i16::MAX)) as i16
        });
    }

    #[test]
    fn measures_loudness_along_the_song() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("song.wav");
        two_part_song(&path, 0.1, 0.8);

        let points = analyse_waveform(&path, &AtomicBool::new(false))
            .unwrap()
            .unwrap();
        assert_eq!(points.len(), WAVEFORM_POINTS);
        assert_eq!(points.iter().max(), Some(&255));
        let (first, second) = points.split_at(WAVEFORM_POINTS / 2);
        let quiet = u32::from(first[WAVEFORM_POINTS / 4]);
        assert!((25..=40).contains(&quiet), "an eighth as loud, not {quiet}");
        assert_eq!(second[WAVEFORM_POINTS / 4], 255);
    }

    #[test]
    fn silence_and_short_songs_still_fill_the_waveform() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("silence.wav");
        two_part_song(&path, 0.0, 0.0);
        let silence = analyse_waveform(&path, &AtomicBool::new(false))
            .unwrap()
            .unwrap();
        assert_eq!(silence, vec![0; WAVEFORM_POINTS]);

        assert_eq!(points(&[]), vec![0; WAVEFORM_POINTS]);
        assert_eq!(points(&[0.25]), vec![255; WAVEFORM_POINTS]);
    }

    #[test]
    fn stops_when_cancelled() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("song.wav");
        two_part_song(&path, 0.5, 0.5);
        assert_eq!(
            analyse_waveform(&path, &AtomicBool::new(true)).unwrap(),
            None
        );
    }

    #[test]
    fn files_that_are_not_audio_fail() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.wav");
        std::fs::write(&path, "not audio").unwrap();
        assert!(analyse_waveform(&path, &AtomicBool::new(false)).is_err());
    }
}
