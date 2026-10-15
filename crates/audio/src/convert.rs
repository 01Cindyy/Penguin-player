//! Converts decoded audio to the output's channel count and sample rate.

use symphonia::core::audio::{Channels, Position};

use crate::resample::Resampler;

/// Gain for centre and surround channels folded into stereo (-3 dB).
const FOLD_GAIN: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// Maps a track's channels and sample rate onto the output stream. When both already match
/// it passes audio through untouched.
pub(crate) struct Converter {
    mixer: ChannelMixer,
    resampler: Option<Resampler>,
    to_rate: u32,
    mixed: Vec<f32>,
}

impl Converter {
    pub fn new(
        layout: &Channels,
        from_rate: u32,
        to_rate: u32,
        to_channels: usize,
    ) -> Result<Self, String> {
        let mixer = ChannelMixer::new(layout, to_channels);
        let resampler = (from_rate != to_rate)
            .then(|| Resampler::new(from_rate, to_rate, to_channels))
            .transpose()?;
        Ok(Self {
            mixer,
            resampler,
            to_rate,
            mixed: Vec::new(),
        })
    }

    /// Whether this converter produces audio in the given output format.
    pub fn produces(&self, rate: u32, channels: usize) -> bool {
        self.to_rate == rate && self.mixer.outputs == channels
    }

    pub fn process<'a>(&'a mut self, input: &'a [f32]) -> &'a [f32] {
        let mixed = if self.mixer.is_passthrough() {
            input
        } else {
            self.mixer.mix(input, &mut self.mixed);
            &self.mixed
        };
        match &mut self.resampler {
            Some(resampler) => resampler.process(mixed),
            None => mixed,
        }
    }

    /// Flushes buffered audio at the end of a track.
    pub fn finish(&mut self) -> &[f32] {
        match &mut self.resampler {
            Some(resampler) => resampler.finish(),
            None => &[],
        }
    }

    /// Drops buffered audio, e.g. after a seek.
    pub fn reset(&mut self) {
        if let Some(resampler) = &mut self.resampler {
            resampler.reset();
        }
    }
}

/// Mixes interleaved audio from one channel layout to another with a gain matrix.
struct ChannelMixer {
    inputs: usize,
    outputs: usize,
    /// `matrix[output * inputs + input]`; empty when the layouts match.
    matrix: Vec<f32>,
}

impl ChannelMixer {
    fn new(layout: &Channels, outputs: usize) -> Self {
        let inputs = layout.count();
        if inputs == outputs {
            return Self {
                inputs,
                outputs,
                matrix: Vec::new(),
            };
        }
        let stereo = stereo_gains(layout);
        let mut matrix = vec![0.0; inputs * outputs];
        for (input, [left, right]) in stereo.into_iter().enumerate() {
            if outputs == 1 {
                matrix[input] = (left + right) / 2.0;
            } else {
                // Extra output channels (surround devices) stay silent.
                matrix[input] = left;
                matrix[inputs + input] = right;
            }
        }
        Self {
            inputs,
            outputs,
            matrix,
        }
    }

    fn is_passthrough(&self) -> bool {
        self.matrix.is_empty()
    }

    fn mix(&self, input: &[f32], output: &mut Vec<f32>) {
        output.clear();
        for frame in input.chunks_exact(self.inputs) {
            for gains in self.matrix.chunks_exact(self.inputs) {
                output.push(
                    frame
                        .iter()
                        .zip(gains)
                        .map(|(sample, gain)| sample * gain)
                        .sum(),
                );
            }
        }
        debug_assert_eq!(output.len() / self.outputs, input.len() / self.inputs);
    }
}

/// How much of each input channel goes to the left and right speaker.
///
/// Mono plays on both speakers at full level. Surround layouts fold centre and surround
/// channels in at -3 dB, drop LFE (as ITU-R BS.775 does), and are scaled so the mix cannot
/// clip.
fn stereo_gains(layout: &Channels) -> Vec<[f32; 2]> {
    let count = layout.count();
    match count {
        1 => return vec![[1.0, 1.0]],
        2 => return vec![[1.0, 0.0], [0.0, 1.0]],
        _ => {}
    }

    let positions = match layout {
        Channels::Positioned(positions) => Some(*positions),
        // Unlabelled channels most likely follow the common WAV order.
        _ => u32::try_from(count)
            .ok()
            .and_then(Position::from_wave_channel_count),
    };
    let mut gains: Vec<[f32; 2]> = match positions {
        Some(positions) => (0..64)
            .map(|bit| Position::from_bits_retain(1 << bit))
            .filter(|position| positions.contains(*position))
            .map(position_gains)
            .collect(),
        // No idea where the channels belong: alternate them between the speakers.
        None => (0..count)
            .map(|index| {
                if index % 2 == 0 {
                    [1.0, 0.0]
                } else {
                    [0.0, 1.0]
                }
            })
            .collect(),
    };
    gains.resize(count, [0.0, 0.0]);

    let loudest = (0..2)
        .map(|side| gains.iter().map(|gain| gain[side]).sum::<f32>())
        .fold(1.0, f32::max);
    for gain in &mut gains {
        gain[0] /= loudest;
        gain[1] /= loudest;
    }
    gains
}

fn position_gains(position: Position) -> [f32; 2] {
    const LEFT: Position = Position::FRONT_LEFT
        .union(Position::FRONT_LEFT_CENTER)
        .union(Position::FRONT_LEFT_WIDE)
        .union(Position::TOP_FRONT_LEFT)
        .union(Position::BOTTOM_FRONT_LEFT);
    const RIGHT: Position = Position::FRONT_RIGHT
        .union(Position::FRONT_RIGHT_CENTER)
        .union(Position::FRONT_RIGHT_WIDE)
        .union(Position::TOP_FRONT_RIGHT)
        .union(Position::BOTTOM_FRONT_RIGHT);
    const LEFT_SURROUND: Position = Position::REAR_LEFT
        .union(Position::SIDE_LEFT)
        .union(Position::TOP_REAR_LEFT)
        .union(Position::TOP_SIDE_LEFT);
    const RIGHT_SURROUND: Position = Position::REAR_RIGHT
        .union(Position::SIDE_RIGHT)
        .union(Position::TOP_REAR_RIGHT)
        .union(Position::TOP_SIDE_RIGHT);
    const LFE: Position = Position::LFE1.union(Position::LFE2);

    if LEFT.contains(position) {
        [1.0, 0.0]
    } else if RIGHT.contains(position) {
        [0.0, 1.0]
    } else if LEFT_SURROUND.contains(position) {
        [FOLD_GAIN, 0.0]
    } else if RIGHT_SURROUND.contains(position) {
        [0.0, FOLD_GAIN]
    } else if LFE.contains(position) {
        [0.0, 0.0]
    } else {
        // Centre channels.
        [FOLD_GAIN, FOLD_GAIN]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mix(layout: Channels, outputs: usize, input: &[f32]) -> Vec<f32> {
        let mut output = Vec::new();
        ChannelMixer::new(&layout, outputs).mix(input, &mut output);
        output
    }

    #[test]
    fn mono_plays_on_both_speakers() {
        let mono = Channels::Positioned(Position::FRONT_CENTER);
        assert_eq!(mix(mono, 2, &[0.5, -0.25]), vec![0.5, 0.5, -0.25, -0.25]);
    }

    #[test]
    fn matching_layouts_pass_through() {
        let stereo = Channels::Positioned(Position::FRONT_LEFT | Position::FRONT_RIGHT);
        assert!(ChannelMixer::new(&stereo, 2).is_passthrough());
    }

    #[test]
    fn surround_folds_down_without_clipping() {
        // 5.1 in WAV order: FL FR FC LFE RL RR, all at full scale.
        let layout = Channels::Discrete(6);
        let output = mix(layout, 2, &[1.0; 6]);
        assert_eq!(output.len(), 2);
        assert!(output.iter().all(|&sample| sample <= 1.0 + f32::EPSILON));
        assert!((output[0] - output[1]).abs() < f32::EPSILON);

        // The LFE channel alone is dropped.
        let lfe_only = mix(Channels::Discrete(6), 2, &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        assert_eq!(lfe_only, vec![0.0, 0.0]);
    }
}
