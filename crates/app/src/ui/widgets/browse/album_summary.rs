//! Facts about an album derived from its tracks, as the album page shows them.

use std::collections::HashSet;

use mp_core::search::normalize;
use mp_core::{AudioFormat, Track};

use crate::ui::widgets::QualityTier;
use crate::util::{song_count, total_length};

/// More distinct qualities than this are summarised as one badge per format.
const MAX_QUALITY_BADGES: usize = 3;

/// A format and quality found on an album, e.g. "FLAC · 24-bit / 96 kHz".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QualityBadge {
    pub format: AudioFormat,
    pub quality: String,
    pub tier: QualityTier,
}

impl QualityBadge {
    pub fn label(&self) -> String {
        if self.quality.is_empty() {
            self.format.label().to_owned()
        } else {
            format!("{} · {}", self.format.label(), self.quality)
        }
    }
}

/// The formats and qualities on an album, best first. Lossless files are told apart
/// by bit depth and sample rate; lossy files only by format, as variable bitrates
/// would give every track its own badge.
pub fn quality_badges<'a>(tracks: impl IntoIterator<Item = &'a Track>) -> Vec<QualityBadge> {
    let mut badges: Vec<QualityBadge> = Vec::new();
    // Each lossy format with the quality all its tracks share, if they do.
    let mut lossy: Vec<(AudioFormat, Option<String>)> = Vec::new();
    for track in tracks {
        let quality = track.quality_label();
        if track.format.is_lossless() {
            let badge = QualityBadge {
                format: track.format,
                quality,
                tier: QualityTier::of(track),
            };
            if !badges.contains(&badge) {
                badges.push(badge);
            }
        } else {
            match lossy.iter_mut().find(|(format, _)| *format == track.format) {
                Some((_, shared)) if shared.as_ref() != Some(&quality) => *shared = None,
                Some(_) => {}
                None => lossy.push((track.format, Some(quality))),
            }
        }
    }
    badges.extend(lossy.into_iter().map(|(format, shared)| QualityBadge {
        format,
        quality: shared.unwrap_or_default(),
        tier: QualityTier::Lossy,
    }));
    // Stable, so qualities of one format and tier keep the order they were found in.
    badges.sort_by(|a, b| b.tier.cmp(&a.tier).then(a.format.cmp(&b.format)));

    if badges.len() > MAX_QUALITY_BADGES {
        // Badges are best first, so each format keeps its best tier.
        let mut formats = HashSet::new();
        badges.retain(|badge| formats.insert(badge.format));
        for badge in &mut badges {
            badge.quality.clear();
        }
    }
    badges
}

/// How many discs the tracks span (at least one).
pub fn disc_count<'a>(tracks: impl IntoIterator<Item = &'a Track>) -> usize {
    let discs: HashSet<u32> = tracks
        .into_iter()
        .map(|track| track.disc_number.unwrap_or(1))
        .collect();
    discs.len().max(1)
}

/// Whether any track is credited to someone other than the album artist, as on
/// compilations or with featured artists. `album_artist` is the album's normalised
/// artist key.
pub fn has_guest_artists<'a>(
    tracks: impl IntoIterator<Item = &'a Track>,
    album_artist: &str,
) -> bool {
    tracks
        .into_iter()
        .any(|track| normalize(track.display_artist()) != album_artist)
}

/// "2023 · 12 songs · 48 min", leaving out an unknown year.
pub fn album_facts(year: u32, track_count: usize, duration_ms: u64) -> String {
    let mut facts = Vec::with_capacity(3);
    if year > 0 {
        facts.push(year.to_string());
    }
    facts.push(song_count(track_count));
    facts.push(total_length(duration_ms));
    facts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::test_track;

    fn track_with(format: AudioFormat, bits: Option<u8>, rate: u32, kbps: Option<u32>) -> Track {
        let mut track = test_track(1, "Song", "Artist", "Album");
        track.format = format;
        track.bit_depth = bits;
        track.sample_rate = Some(rate);
        track.bitrate_kbps = kbps;
        track
    }

    #[test]
    fn badges_describe_each_quality_best_first() {
        let tracks = [
            track_with(AudioFormat::Flac, Some(16), 44_100, None),
            track_with(AudioFormat::Flac, Some(24), 96_000, None),
            track_with(AudioFormat::Flac, Some(16), 44_100, None),
        ];
        let badges = quality_badges(&tracks);
        let labels: Vec<String> = badges.iter().map(QualityBadge::label).collect();
        assert_eq!(
            labels,
            ["FLAC · 24-bit / 96 kHz", "FLAC · 16-bit / 44.1 kHz"]
        );
        assert_eq!(badges[0].tier, QualityTier::HiRes);
        assert_eq!(badges[1].tier, QualityTier::Lossless);
    }

    #[test]
    fn lossy_badges_group_by_format() {
        let uniform = [
            track_with(AudioFormat::Mp3, None, 44_100, Some(320)),
            track_with(AudioFormat::Mp3, None, 44_100, Some(320)),
        ];
        assert_eq!(quality_badges(&uniform)[0].label(), "MP3 · 320 kbps");

        let variable = [
            track_with(AudioFormat::Mp3, None, 44_100, Some(245)),
            track_with(AudioFormat::Mp3, None, 44_100, Some(251)),
        ];
        let badges = quality_badges(&variable);
        assert_eq!(badges.len(), 1);
        assert_eq!(badges[0].label(), "MP3");
        assert_eq!(badges[0].tier, QualityTier::Lossy);
    }

    #[test]
    fn many_qualities_collapse_to_formats() {
        let tracks = [
            track_with(AudioFormat::Flac, Some(16), 44_100, None),
            track_with(AudioFormat::Flac, Some(24), 96_000, None),
            track_with(AudioFormat::Flac, Some(24), 192_000, None),
            track_with(AudioFormat::Alac, Some(16), 48_000, None),
        ];
        let labels: Vec<String> = quality_badges(&tracks)
            .iter()
            .map(QualityBadge::label)
            .collect();
        assert_eq!(labels, ["FLAC", "ALAC"]);
    }

    #[test]
    fn discs_artists_and_facts() {
        let mut first = test_track(1, "One", "Band", "Record");
        let mut second = test_track(2, "Two", "Guest", "Record");
        assert_eq!(disc_count([&first, &second]), 1);
        second.disc_number = Some(2);
        first.disc_number = Some(1);
        assert_eq!(disc_count([&first, &second]), 2);
        assert!(has_guest_artists([&first, &second], "band"));
        assert!(!has_guest_artists([&first], "band"));
        let mut compilation = test_track(3, "Three", "Solo", "Hits");
        compilation.album_artist = "Various Artists".into();
        assert!(has_guest_artists([&compilation], "various artists"));

        assert_eq!(
            album_facts(2023, 12, 48 * 60_000),
            "2023 · 12 songs · 48 min"
        );
        assert_eq!(album_facts(0, 1, 200_000), "1 song · 3 min");
        assert_eq!(album_facts(0, 1, 15_000), "1 song · 0:15");
    }
}
