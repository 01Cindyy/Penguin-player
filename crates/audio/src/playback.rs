//! A track opened for playback: its decoder plus the conversion to the output's format.

use std::path::{Path, PathBuf};
use std::time::Duration;

use mp_core::TrackId;

use crate::convert::Converter;
use crate::decoder::{DecodeError, TrackDecoder};
use crate::output::Output;

pub(crate) struct Playback {
    pub track: TrackId,
    path: PathBuf,
    decoder: TrackDecoder,
    /// Built for the output format the audio last went to.
    converter: Option<Converter>,
    /// All of the track's audio has been written (or decoding failed).
    finished: bool,
}

impl Playback {
    pub fn open(track: TrackId, path: PathBuf) -> Result<Self, DecodeError> {
        let decoder = TrackDecoder::open(&path)?;
        Ok(Self {
            track,
            path,
            decoder,
            converter: None,
            finished: false,
        })
    }

    pub fn is(&self, track: TrackId, path: &Path) -> bool {
        self.track == track && self.path == path
    }

    pub fn duration(&self) -> Option<Duration> {
        self.decoder.info().duration
    }

    pub fn sample_rate(&self) -> u32 {
        self.decoder.info().sample_rate
    }

    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// Decodes the next block of audio into `output`. At the end of the track the
    /// converter's tail is written too and the playback is marked finished; a decoding
    /// error also finishes it.
    pub fn decode_into(&mut self, output: &mut Output) -> Result<(), DecodeError> {
        let format = output.format();
        let channels = usize::from(format.channels);
        let converter = match self.converter.take() {
            Some(converter) if converter.produces(format.sample_rate, channels) => converter,
            _ => match Converter::new(
                self.decoder.layout(),
                self.decoder.info().sample_rate,
                format.sample_rate,
                channels,
            ) {
                Ok(converter) => converter,
                Err(error) => {
                    self.finished = true;
                    return Err(DecodeError::Unsupported(error));
                }
            },
        };
        let converter = self.converter.insert(converter);

        match self.decoder.next_chunk() {
            Ok(Some(samples)) => output.writer.write(converter.process(samples)),
            Ok(None) => {
                output.writer.write(converter.finish());
                self.finished = true;
            }
            Err(error) => {
                self.finished = true;
                return Err(error);
            }
        }
        Ok(())
    }

    /// Seeks so decoding continues at `position`. Returns the position actually reached.
    /// Seeking to or past the end makes the track finish (and the next one follow).
    pub fn seek(&mut self, position: Duration) -> Result<Duration, DecodeError> {
        let reached = self.decoder.seek(position)?;
        if let Some(converter) = &mut self.converter {
            converter.reset();
        }
        self.finished = false;
        Ok(reached)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cpal::SampleFormat;

    use super::*;
    use crate::output::{OutputFormat, Sink};
    use crate::shared::Shared;
    use crate::test_files;

    #[test]
    fn a_track_that_cannot_be_converted_is_finished() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("a.wav");
        test_files::write_wav(&path, 8_000, 2, 800, test_files::tone(8_000));
        let mut playback = Playback::open(TrackId(1), path).expect("opens");
        // No resampler can produce audio at 0 Hz.
        let format = OutputFormat {
            sample_rate: 0,
            channels: 2,
            sample_format: SampleFormat::F32,
        };
        let shared = Arc::new(Shared::new(1.0));
        let mut output = Sink::Null
            .open_output(format, &shared, 1, |_, _| {})
            .expect("null output");

        assert!(playback.decode_into(&mut output).is_err());
        // An unfinished track would make the engine try to decode it again straight away.
        assert!(playback.is_finished());
    }
}
