//! What the library is made of: totals per format, quality tiers and the quality
//! profiles tracks share, all gathered in one pass over the tracks.

use std::cmp::Reverse;
use std::collections::HashMap;

use mp_core::{AudioFormat, Track};

use crate::ui::widgets::QualityTier;

/// Lossy bitrates at or above this many kbps are in the top band.
const HIGH_BITRATE_KBPS: u32 = 256;
/// Lossy bitrates below this many kbps are in the bottom band.
const LOW_BITRATE_KBPS: u32 = 160;

/// Bitrate bands that group lossy tracks, highest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum BitrateBand {
    High,
    Medium,
    Low,
    Unknown,
}

impl BitrateBand {
    fn of(kbps: Option<u32>) -> Self {
        match kbps {
            Some(kbps) if kbps >= HIGH_BITRATE_KBPS => BitrateBand::High,
            Some(kbps) if kbps >= LOW_BITRATE_KBPS => BitrateBand::Medium,
            Some(_) => BitrateBand::Low,
            None => BitrateBand::Unknown,
        }
    }

    fn label(self) -> String {
        match self {
            BitrateBand::High => format!("{HIGH_BITRATE_KBPS}+ kbps"),
            BitrateBand::Medium => {
                format!("{LOW_BITRATE_KBPS}–{} kbps", HIGH_BITRATE_KBPS - 1)
            }
            BitrateBand::Low => format!("Under {LOW_BITRATE_KBPS} kbps"),
            BitrateBand::Unknown => "Unknown Bitrate".to_owned(),
        }
    }
}

/// A track's stream quality, coarse enough to group tracks by: bit depth and sample
/// rate for lossless formats, a bitrate band for lossy ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Quality {
    Lossless {
        bit_depth: Option<u8>,
        sample_rate: Option<u32>,
    },
    Lossy(BitrateBand),
}

impl Quality {
    pub fn of(track: &Track) -> Self {
        if track.format.is_lossless() {
            Quality::Lossless {
                bit_depth: track.bit_depth,
                sample_rate: track.sample_rate,
            }
        } else {
            Quality::Lossy(BitrateBand::of(track.bitrate_kbps))
        }
    }

    /// Compact label for chips: "24/96", "16/44.1" or a bitrate band.
    pub fn short_label(self) -> String {
        match self {
            Quality::Lossless {
                bit_depth: Some(bits),
                sample_rate: Some(rate),
            } => format!("{bits}/{}", khz(rate)),
            Quality::Lossless { .. } | Quality::Lossy(_) => self.label(),
        }
    }

    /// Full label: "24-bit / 96 kHz", "16-bit", "160–255 kbps".
    pub fn label(self) -> String {
        match self {
            Quality::Lossless {
                bit_depth,
                sample_rate,
            } => match (bit_depth, sample_rate) {
                (Some(bits), Some(rate)) => format!("{bits}-bit / {} kHz", khz(rate)),
                (Some(bits), None) => format!("{bits}-bit"),
                (None, Some(rate)) => format!("{} kHz", khz(rate)),
                (None, None) => "Unknown Resolution".to_owned(),
            },
            Quality::Lossy(band) => band.label(),
        }
    }

    /// Orders qualities best first: higher resolutions, then higher bitrates.
    fn rank(self) -> impl Ord {
        match self {
            Quality::Lossless {
                bit_depth,
                sample_rate,
            } => (
                0,
                Reverse(bit_depth),
                Reverse(sample_rate),
                BitrateBand::High,
            ),
            Quality::Lossy(band) => (1, Reverse(None), Reverse(None), band),
        }
    }
}

/// Totals for one format.
#[derive(Debug, Clone, PartialEq)]
pub struct FormatStats {
    pub format: AudioFormat,
    pub tracks: usize,
    pub duration_ms: u64,
    pub bytes: u64,
    /// How many tracks have each quality.
    qualities: HashMap<Quality, usize>,
    /// Lowest and highest bitrate of lossy tracks that report one.
    bitrates: Option<(u32, u32)>,
}

impl FormatStats {
    fn new(format: AudioFormat) -> Self {
        Self {
            format,
            tracks: 0,
            duration_ms: 0,
            bytes: 0,
            qualities: HashMap::new(),
            bitrates: None,
        }
    }

    fn add(&mut self, track: &Track) {
        self.tracks += 1;
        self.duration_ms += track.duration_ms;
        self.bytes += track.file_size;
        *self.qualities.entry(Quality::of(track)).or_default() += 1;
        if !track.format.is_lossless()
            && let Some(kbps) = track.bitrate_kbps
        {
            self.bitrates = Some(match self.bitrates {
                Some((low, high)) => (low.min(kbps), high.max(kbps)),
                None => (kbps, kbps),
            });
        }
    }

    /// The qualities present, best first, with how many tracks have each.
    pub fn qualities(&self) -> Vec<(Quality, usize)> {
        let mut qualities: Vec<(Quality, usize)> =
            self.qualities.iter().map(|(&q, &n)| (q, n)).collect();
        qualities.sort_by_key(|&(quality, _)| quality.rank());
        qualities
    }

    /// What a track in this format usually is: "24-bit / 96 kHz", "Mostly 16-bit /
    /// 44.1 kHz" when resolutions vary, or the bitrate range of lossy formats.
    pub fn typical_quality(&self) -> String {
        if !self.format.is_lossless() {
            return match self.bitrates {
                Some((low, high)) if low == high => format!("{low} kbps"),
                Some((low, high)) => format!("{low}–{high} kbps"),
                None => "Unknown bitrate".to_owned(),
            };
        }
        // Best first, so ties go to the better quality.
        let most_common = self
            .qualities()
            .into_iter()
            .reduce(|best, next| if next.1 > best.1 { next } else { best })
            .map(|(quality, _)| quality.label());
        match most_common {
            Some(label) if self.qualities.len() == 1 => label,
            Some(label) => format!("Mostly {label}"),
            None => "Unknown".to_owned(),
        }
    }
}

/// The library's make-up.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Composition {
    /// Formats present in the library, in [`AudioFormat::ALL`] order.
    pub formats: Vec<FormatStats>,
    pub tracks: usize,
    pub duration_ms: u64,
    pub bytes: u64,
    /// Tracks per [`QualityTier`], indexed by the tier.
    tiers: [usize; 3],
}

impl Composition {
    pub fn of<'a>(tracks: impl IntoIterator<Item = &'a Track>) -> Self {
        let mut formats: HashMap<AudioFormat, FormatStats> = HashMap::new();
        let mut composition = Composition::default();
        for track in tracks {
            composition.tracks += 1;
            composition.duration_ms += track.duration_ms;
            composition.bytes += track.file_size;
            composition.tiers[QualityTier::of(track) as usize] += 1;
            formats
                .entry(track.format)
                .or_insert_with(|| FormatStats::new(track.format))
                .add(track);
        }
        composition.formats = AudioFormat::ALL
            .into_iter()
            .filter_map(|format| formats.remove(&format))
            .collect();
        composition
    }

    /// How many tracks are in `tier`.
    pub fn tier(&self, tier: QualityTier) -> usize {
        self.tiers[tier as usize]
    }

    /// The formats, most tracks first.
    pub fn by_popularity(&self) -> Vec<&FormatStats> {
        let mut formats: Vec<&FormatStats> = self.formats.iter().collect();
        formats.sort_by_key(|stats| Reverse(stats.tracks));
        formats
    }

    pub fn format(&self, format: AudioFormat) -> Option<&FormatStats> {
        self.formats.iter().find(|stats| stats.format == format)
    }

    /// `count` as a share of all tracks, e.g. "41%", "<1%" for tiny shares.
    pub fn percent(&self, count: usize) -> String {
        percent(count, self.tracks)
    }

    /// `count` out of all tracks as a fraction in `0.0..=1.0`.
    pub fn fraction(&self, count: usize) -> f64 {
        if self.tracks == 0 {
            0.0
        } else {
            count as f64 / self.tracks as f64
        }
    }
}

/// `part` of `whole` as a rounded percentage; never "0%" for a part that exists.
pub fn percent(part: usize, whole: usize) -> String {
    if whole == 0 || part == 0 {
        return "0%".to_owned();
    }
    match (part * 100 + whole / 2) / whole {
        0 => "<1%".to_owned(),
        value => format!("{value}%"),
    }
}

/// A sample rate in kHz without needless decimals: 96, 44.1, 88.2.
fn khz(rate: u32) -> String {
    if rate.is_multiple_of(1000) {
        (rate / 1000).to_string()
    } else {
        format!("{:.1}", f64::from(rate) / 1000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::test_track;

    fn lossless(id: i64, format: AudioFormat, bits: u8, rate: u32) -> Track {
        let mut track = test_track(id, "t", "a", "b");
        track.format = format;
        track.bit_depth = Some(bits);
        track.sample_rate = Some(rate);
        track.file_size = 1000;
        track
    }

    fn lossy(id: i64, format: AudioFormat, kbps: Option<u32>) -> Track {
        let mut track = test_track(id, "t", "a", "b");
        track.format = format;
        track.bit_depth = None;
        track.sample_rate = Some(44_100);
        track.bitrate_kbps = kbps;
        track.file_size = 100;
        track
    }

    #[test]
    fn composition_totals_and_tiers() {
        let tracks = vec![
            lossy(1, AudioFormat::Mp3, Some(320)),
            lossless(2, AudioFormat::Flac, 24, 96_000),
            lossless(3, AudioFormat::Flac, 16, 44_100),
            lossless(4, AudioFormat::Alac, 16, 44_100),
            lossy(5, AudioFormat::Mp3, Some(128)),
        ];
        let composition = Composition::of(&tracks);
        assert_eq!(composition.tracks, 5);
        assert_eq!(composition.bytes, 3200);
        assert_eq!(composition.tier(QualityTier::HiRes), 1);
        assert_eq!(composition.tier(QualityTier::Lossless), 2);
        assert_eq!(composition.tier(QualityTier::Lossy), 2);

        let order: Vec<AudioFormat> = composition.formats.iter().map(|f| f.format).collect();
        assert_eq!(
            order,
            [AudioFormat::Flac, AudioFormat::Alac, AudioFormat::Mp3]
        );
        let popular = composition.by_popularity();
        assert_eq!(popular[2].format, AudioFormat::Alac);
        assert_eq!(composition.percent(2), "40%");
    }

    #[test]
    fn typical_quality() {
        let tracks = vec![
            lossless(1, AudioFormat::Flac, 24, 96_000),
            lossless(2, AudioFormat::Flac, 24, 96_000),
            lossless(3, AudioFormat::Flac, 16, 44_100),
            lossless(4, AudioFormat::Alac, 16, 44_100),
            lossy(5, AudioFormat::Mp3, Some(320)),
            lossy(6, AudioFormat::Mp3, Some(192)),
            lossy(7, AudioFormat::Opus, None),
        ];
        let composition = Composition::of(&tracks);
        let typical = |format| composition.format(format).unwrap().typical_quality();
        assert_eq!(typical(AudioFormat::Flac), "Mostly 24-bit / 96 kHz");
        assert_eq!(typical(AudioFormat::Alac), "16-bit / 44.1 kHz");
        assert_eq!(typical(AudioFormat::Mp3), "192–320 kbps");
        assert_eq!(typical(AudioFormat::Opus), "Unknown bitrate");
    }

    #[test]
    fn qualities_sort_best_first() {
        let tracks = vec![
            lossless(1, AudioFormat::Flac, 16, 44_100),
            lossless(2, AudioFormat::Flac, 24, 192_000),
            lossless(3, AudioFormat::Flac, 24, 96_000),
            lossless(4, AudioFormat::Flac, 16, 44_100),
        ];
        let composition = Composition::of(&tracks);
        let labels: Vec<(String, usize)> = composition
            .format(AudioFormat::Flac)
            .unwrap()
            .qualities()
            .into_iter()
            .map(|(quality, count)| (quality.short_label(), count))
            .collect();
        assert_eq!(
            labels,
            [
                ("24/192".to_owned(), 1),
                ("24/96".to_owned(), 1),
                ("16/44.1".to_owned(), 2)
            ]
        );
    }

    #[test]
    fn quality_labels() {
        let band = |kbps| Quality::Lossy(BitrateBand::of(kbps)).label();
        assert_eq!(band(Some(320)), "256+ kbps");
        assert_eq!(band(Some(192)), "160–255 kbps");
        assert_eq!(band(Some(96)), "Under 160 kbps");
        assert_eq!(band(None), "Unknown Bitrate");
        let partial = Quality::Lossless {
            bit_depth: None,
            sample_rate: Some(88_200),
        };
        assert_eq!(partial.short_label(), "88.2 kHz");
    }

    #[test]
    fn percentages() {
        assert_eq!(percent(0, 10), "0%");
        assert_eq!(percent(1, 1000), "<1%");
        assert_eq!(percent(1, 3), "33%");
        assert_eq!(percent(2, 3), "67%");
        assert_eq!(percent(5, 0), "0%");
    }
}
