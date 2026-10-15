//! Bar heights for a spectrum visualiser, in the spirit of cava: the audio is split into
//! frequency bands spaced the way hearing is (as many bars for 100 to 200 Hz as for 1 to
//! 2 kHz), levels are shown in decibels relative to the recent peak so quiet and loud
//! songs both fill the space, and bars jump up at once and fall back under gravity.

use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};

/// Audio analysed per update. 2048 frames at 44.1 kHz is 46 ms: enough to tell bass notes
/// apart while still following the beat.
const FFT_FRAMES: usize = 2048;
/// Higher sample rates are averaged down to about this first, so every rate gets the same
/// frequency resolution.
const ANALYSIS_RATE: u32 = 48_000;

const LOWEST_HZ: f32 = 50.0;
const HIGHEST_HZ: f32 = 10_000.0;
/// Music is quieter the higher it goes; this tilt (per octave, around 1 kHz) evens the
/// bars out so treble isn't always flat.
const TILT_DB_PER_OCTAVE: f32 = 3.0;
/// Decibels between an empty bar and the recent peak.
const RANGE_DB: f32 = 45.0;
/// The peak reference falls this fast after a loud moment, so the bars adapt to quieter
/// songs within a few seconds (cava's "autosens").
const REFERENCE_FALL_DB_PER_SECOND: f32 = 6.0;
/// Below this the audio is near silence, and isn't amplified to fill the bars.
const QUIETEST_REFERENCE_DB: f32 = -50.0;
/// How quickly bars rise to a louder level, per second.
const RISE_RATE: f32 = 35.0;
/// How quickly falling bars accelerate, in bar heights per second squared.
const GRAVITY: f32 = 4.5;

/// Turns audio into smoothed bar heights in `0.0..=1.0`. Keep one per visualiser and call
/// [`Self::update`] once per frame.
pub struct Spectrum {
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    input: Vec<f32>,
    output: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    magnitudes: Vec<f32>,
    /// Each bar's level before smoothing; kept to reuse its memory every frame.
    targets: Vec<f32>,
    levels: Vec<f32>,
    fall_speeds: Vec<f32>,
    reference_db: f32,
}

impl Default for Spectrum {
    fn default() -> Self {
        Self::new()
    }
}

impl Spectrum {
    pub fn new() -> Self {
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(FFT_FRAMES);
        let window = (0..FFT_FRAMES)
            .map(|index| {
                let phase = std::f32::consts::TAU * index as f32 / FFT_FRAMES as f32;
                0.5 - 0.5 * phase.cos()
            })
            .collect();
        Self {
            input: fft.make_input_vec(),
            output: fft.make_output_vec(),
            scratch: fft.make_scratch_vec(),
            magnitudes: vec![0.0; FFT_FRAMES / 2 + 1],
            fft,
            window,
            targets: Vec::new(),
            levels: Vec::new(),
            fall_speeds: Vec::new(),
            reference_db: QUIETEST_REFERENCE_DB,
        }
    }

    /// How many mono samples [`Self::update`] wants at `sample_rate`.
    pub fn frames_wanted(sample_rate: u32) -> usize {
        FFT_FRAMES * decimation(sample_rate)
    }

    /// Advances the bars by `elapsed` seconds towards the levels in `audio` (mono samples
    /// at `sample_rate`, see [`Self::frames_wanted`]), or towards silence for `None`.
    pub fn update(&mut self, audio: Option<(&[f32], u32)>, bars: usize, elapsed: f32) -> &[f32] {
        if self.levels.len() != bars {
            self.levels = vec![0.0; bars];
            self.fall_speeds = vec![0.0; bars];
        }
        self.targets.clear();
        match audio {
            Some((samples, sample_rate)) if sample_rate > 0 => {
                self.analyse(samples, sample_rate);
                let analysis_rate = sample_rate / decimation(sample_rate) as u32;
                self.bar_targets(analysis_rate, bars, elapsed);
            }
            _ => self.targets.resize(bars, 0.0),
        }
        for ((level, speed), &target) in self
            .levels
            .iter_mut()
            .zip(&mut self.fall_speeds)
            .zip(&self.targets)
        {
            if target >= *level {
                *level += (target - *level) * (RISE_RATE * elapsed).min(1.0);
                *speed = 0.0;
            } else {
                *speed += GRAVITY * elapsed;
                *level = (*level - *speed * elapsed).max(target);
            }
        }
        &self.levels
    }

    /// Whether every bar has come to rest at the bottom.
    pub fn is_silent(&self) -> bool {
        self.levels.iter().all(|&level| level <= 0.001)
    }

    /// Fills `magnitudes` with the spectrum of the newest audio, scaled so a full-scale
    /// sine wave peaks at 1.0.
    fn analyse(&mut self, samples: &[f32], sample_rate: u32) {
        let step = decimation(sample_rate);
        // Short audio (just after playback starts) is padded with silence at the front.
        let frames = FFT_FRAMES.min(samples.len() / step);
        let missing = FFT_FRAMES - frames;
        let samples = &samples[samples.len() - frames * step..];
        for (index, (value, weight)) in self.input.iter_mut().zip(&self.window).enumerate() {
            *value = match index.checked_sub(missing) {
                Some(frame) => {
                    let group = &samples[frame * step..(frame + 1) * step];
                    group.iter().sum::<f32>() / step as f32 * weight
                }
                None => 0.0,
            };
        }
        if self
            .fft
            .process_with_scratch(&mut self.input, &mut self.output, &mut self.scratch)
            .is_err()
        {
            self.magnitudes.fill(0.0);
            return;
        }
        let scale = 4.0 / FFT_FRAMES as f32;
        for (magnitude, bin) in self.magnitudes.iter_mut().zip(&self.output) {
            *magnitude = bin.norm() * scale;
        }
    }

    /// Sets `targets` to each bar's level from the spectrum, before smoothing.
    fn bar_targets(&mut self, analysis_rate: u32, bars: usize, elapsed: f32) {
        let bin_hz = analysis_rate as f32 / FFT_FRAMES as f32;
        let highest = HIGHEST_HZ.min(analysis_rate as f32 * 0.45);
        let edge = |bar: usize| LOWEST_HZ * (highest / LOWEST_HZ).powf(bar as f32 / bars as f32);
        // Levels in decibels first, then relative to the recent peak.
        let mut targets = std::mem::take(&mut self.targets);
        targets.extend((0..bars).map(|bar| {
            let (low, high) = (edge(bar), edge(bar + 1));
            let centre = (low * high).sqrt();
            let magnitude = self.band_magnitude(low / bin_hz, high / bin_hz);
            20.0 * magnitude.max(1e-9).log10() + TILT_DB_PER_OCTAVE * (centre / 1000.0).log2()
        }));

        let peak = targets.iter().copied().fold(f32::MIN, f32::max);
        self.reference_db = (self.reference_db - REFERENCE_FALL_DB_PER_SECOND * elapsed)
            .max(peak)
            .max(QUIETEST_REFERENCE_DB);
        let floor = self.reference_db - RANGE_DB;
        for target in &mut targets {
            *target = ((*target - floor) / RANGE_DB).clamp(0.0, 1.0);
        }
        self.targets = targets;
    }

    /// The loudest bin between fractional bin positions `low` and `high`, or the level
    /// interpolated between the two nearest bins when the band is narrower than one.
    fn band_magnitude(&self, low: f32, high: f32) -> f32 {
        let last = self.magnitudes.len() - 1;
        let first_bin = low.ceil() as usize;
        let last_bin = (high.floor() as usize).min(last);
        if first_bin <= last_bin {
            return self.magnitudes[first_bin..=last_bin]
                .iter()
                .copied()
                .fold(0.0, f32::max);
        }
        let position = ((low + high) / 2.0).min(last as f32);
        let below = position.floor() as usize;
        let above = (below + 1).min(last);
        let fraction = position - below as f32;
        self.magnitudes[below] * (1.0 - fraction) + self.magnitudes[above] * fraction
    }
}

/// How many samples are averaged into one for analysis.
fn decimation(sample_rate: u32) -> usize {
    ((sample_rate + ANALYSIS_RATE / 2) / ANALYSIS_RATE).max(1) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: f32 = 1.0 / 60.0;

    fn sine(hz: f32, sample_rate: u32, amplitude: f32) -> Vec<f32> {
        (0..Spectrum::frames_wanted(sample_rate))
            .map(|index| {
                amplitude * (std::f32::consts::TAU * hz * index as f32 / sample_rate as f32).sin()
            })
            .collect()
    }

    /// The bars after a second of steady audio.
    fn settled(audio: &[f32], sample_rate: u32, bars: usize) -> Vec<f32> {
        let mut spectrum = Spectrum::new();
        for _ in 0..60 {
            spectrum.update(Some((audio, sample_rate)), bars, FRAME);
        }
        spectrum.levels.clone()
    }

    fn loudest(levels: &[f32]) -> usize {
        levels
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(index, _)| index)
            .unwrap()
    }

    #[test]
    fn a_tone_lights_up_the_bar_for_its_frequency() {
        let bars = 32;
        let low = settled(&sine(100.0, 44_100, 0.5), 44_100, bars);
        let high = settled(&sine(4_000.0, 44_100, 0.5), 44_100, bars);
        assert!(loudest(&low) < bars / 4, "{low:?}");
        assert!(loudest(&high) > bars * 3 / 4, "{high:?}");
        assert!(low[loudest(&low)] > 0.9);
        // Bars far from the tone stay low.
        assert!(low[bars - 1] < 0.3, "{low:?}");
    }

    #[test]
    fn high_sample_rates_put_tones_in_the_same_place() {
        let bars = 24;
        let cd = settled(&sine(1_000.0, 44_100, 0.5), 44_100, bars);
        let hi_res = settled(&sine(1_000.0, 192_000, 0.5), 192_000, bars);
        assert!(loudest(&cd).abs_diff(loudest(&hi_res)) <= 1);
    }

    #[test]
    fn quiet_songs_fill_the_bars_too() {
        let loud = settled(&sine(440.0, 48_000, 0.8), 48_000, 16);
        let quiet = settled(&sine(440.0, 48_000, 0.05), 48_000, 16);
        assert!((loud[loudest(&loud)] - quiet[loudest(&quiet)]).abs() < 0.05);
    }

    #[test]
    fn silence_stays_empty_and_bars_fall_under_gravity() {
        assert!(
            settled(&vec![0.0; 4096], 44_100, 16)
                .iter()
                .all(|&l| l == 0.0)
        );

        let tone = sine(440.0, 44_100, 0.5);
        let mut spectrum = Spectrum::new();
        for _ in 0..30 {
            spectrum.update(Some((&tone, 44_100)), 16, FRAME);
        }
        assert!(!spectrum.is_silent());
        let peak = loudest(&spectrum.levels);
        let mut heights = Vec::new();
        for _ in 0..90 {
            heights.push(spectrum.update(None, 16, FRAME)[peak]);
        }
        let first_drop = 1.0 - heights[0];
        let later_drop = heights[9] - heights[10];
        assert!(later_drop > first_drop, "falls faster as it goes");
        assert!(spectrum.is_silent());
    }

    #[test]
    fn short_or_missing_audio_and_new_bar_counts_are_handled() {
        let mut spectrum = Spectrum::new();
        assert_eq!(
            spectrum.update(Some((&[0.5; 10], 44_100)), 8, FRAME).len(),
            8
        );
        assert_eq!(spectrum.update(Some((&[], 0)), 12, FRAME).len(), 12);
        assert_eq!(spectrum.update(None, 0, FRAME).len(), 0);
        assert_eq!(decimation(44_100), 1);
        assert_eq!(decimation(96_000), 2);
        assert_eq!(decimation(192_000), 4);
    }
}
