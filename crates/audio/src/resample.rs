//! Sample rate conversion for devices that cannot play a track's native rate.

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler as _};

/// Input frames per resampler call. Larger chunks are cheaper per frame but add latency.
const CHUNK_FRAMES: usize = 1024;

/// Streams interleaved audio through a high quality FFT resampler.
///
/// Input arrives in arbitrary block sizes and is regrouped into the fixed chunks rubato
/// needs. The resampler's startup delay is trimmed and its tail is flushed by
/// [`Resampler::finish`], so tracks keep their exact length.
pub(crate) struct Resampler {
    inner: Fft<f32>,
    channels: usize,
    ratio: f64,
    /// Input that has not filled a whole chunk yet.
    pending: Vec<f32>,
    output: Vec<f32>,
    /// Output frames still to drop to compensate for the resampler's delay.
    delay_left: usize,
    frames_in: u64,
    frames_out: u64,
}

impl Resampler {
    pub fn new(from_rate: u32, to_rate: u32, channels: usize) -> Result<Self, String> {
        let inner = Fft::new(
            from_rate as usize,
            to_rate as usize,
            CHUNK_FRAMES,
            channels,
            FixedSync::Input,
        )
        .map_err(|error| error.to_string())?;
        let mut resampler = Self {
            channels,
            ratio: f64::from(to_rate) / f64::from(from_rate),
            pending: Vec::with_capacity(CHUNK_FRAMES * channels),
            output: Vec::new(),
            delay_left: 0,
            frames_in: 0,
            frames_out: 0,
            inner,
        };
        resampler.reset();
        Ok(resampler)
    }

    /// Clears all state, e.g. after a seek.
    pub fn reset(&mut self) {
        self.inner.reset();
        self.pending.clear();
        self.delay_left = self.inner.output_delay();
        self.frames_in = 0;
        self.frames_out = 0;
    }

    /// Resamples `input` and returns whatever output is ready.
    pub fn process(&mut self, mut input: &[f32]) -> &[f32] {
        self.output.clear();
        let chunk_len = CHUNK_FRAMES * self.channels;
        while !input.is_empty() {
            let take = (chunk_len - self.pending.len()).min(input.len());
            self.pending.extend_from_slice(&input[..take]);
            input = &input[take..];
            if self.pending.len() == chunk_len {
                self.run_chunk(CHUNK_FRAMES);
                self.pending.clear();
            }
        }
        &self.output
    }

    /// Resamples the remaining input and flushes the resampler, returning the last output.
    pub fn finish(&mut self) -> &[f32] {
        self.output.clear();
        let mut partial = self.pending.len() / self.channels;
        self.frames_in += partial as u64;
        let expected = (self.frames_in as f64 * self.ratio).round() as u64;
        // Feed silence until the delayed tail of the real input has come out. When the rates
        // need FFT blocks longer than a chunk, some chunks produce nothing at all.
        while self.frames_out < expected {
            self.pending.resize(CHUNK_FRAMES * self.channels, 0.0);
            if !self.run_partial_chunk(partial) {
                break;
            }
            partial = 0;
        }
        let excess = (self.frames_out.saturating_sub(expected)) as usize * self.channels;
        self.output
            .truncate(self.output.len().saturating_sub(excess));
        self.reset();
        &self.output
    }

    fn run_chunk(&mut self, frames: usize) {
        self.frames_in += frames as u64;
        self.run_partial_chunk(frames);
    }

    /// Runs one resampler call on `pending`, of which the first `frames` are real input.
    /// Returns false if the resampler failed.
    fn run_partial_chunk(&mut self, frames: usize) -> bool {
        let start = self.output.len();
        let out_frames = self.inner.output_frames_max();
        self.output.resize(start + out_frames * self.channels, 0.0);

        let input = InterleavedSlice::new(&self.pending, self.channels, CHUNK_FRAMES);
        let output =
            InterleavedSlice::new_mut(&mut self.output[start..], self.channels, out_frames);
        let indexing = Indexing::new().partial_len(frames);
        let produced = match (input, output) {
            (Ok(input), Ok(mut output)) => self
                .inner
                .process_into_buffer(&input, &mut output, Some(&indexing))
                .map(|(_, produced)| produced)
                .map_err(|error| error.to_string()),
            _ => Err("buffers do not match the resampler".to_owned()),
        };
        let produced = match produced {
            Ok(produced) => produced,
            Err(error) => {
                log::warn!("resampler failed: {error}");
                self.output.truncate(start);
                return false;
            }
        };

        // Drop the startup delay so the output lines up with the input.
        let trim = self.delay_left.min(produced);
        self.delay_left -= trim;
        self.output.truncate(start + produced * self.channels);
        self.output.drain(start..start + trim * self.channels);
        self.frames_out += (produced - trim) as u64;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Resamples `seconds` of a stereo 440 Hz sine, fed in odd-sized blocks, and checks that
    /// the output has exactly the expected length and is the same sine. Rubato rounds its
    /// delay to whole frames, so the output may be shifted by half a frame.
    fn check_tone(from: u32, to: u32, seconds: f64) {
        let sine = |frame: f64, rate: u32| {
            (frame * 440.0 * std::f64::consts::TAU / f64::from(rate)).sin() as f32
        };
        let in_frames = (f64::from(from) * seconds) as usize;
        let input: Vec<f32> = (0..in_frames)
            .flat_map(|frame| [sine(frame as f64, from); 2])
            .collect();

        let mut resampler = Resampler::new(from, to, 2).expect("valid rates");
        let mut output = Vec::new();
        for block in input.chunks(2 * 700) {
            output.extend_from_slice(resampler.process(block));
        }
        output.extend_from_slice(resampler.finish());

        let out_frames = (in_frames as f64 * f64::from(to) / f64::from(from)).round() as usize;
        assert_eq!(output.len(), 2 * out_frames, "{from} Hz -> {to} Hz");
        let middle = out_frames / 4..out_frames * 3 / 4;
        let matches_sine = [-0.5, 0.0, 0.5].into_iter().any(|shift| {
            middle
                .clone()
                .all(|frame| (output[frame * 2] - sine(frame as f64 + shift, to)).abs() < 0.01)
        });
        assert!(matches_sine, "{from} Hz -> {to} Hz is not the same sine");
    }

    #[test]
    fn keeps_length_and_tone() {
        check_tone(44_100, 48_000, 1.0);
        check_tone(96_000, 44_100, 0.5);
    }

    #[test]
    fn flushes_the_whole_tail_when_blocks_exceed_a_chunk() {
        // These rate pairs need FFT blocks longer than a chunk, so at the end of the track
        // the resampler still holds more than a chunk of input.
        check_tone(384_000, 44_100, 0.5);
        check_tone(44_101, 48_000, 0.1);
    }
}
