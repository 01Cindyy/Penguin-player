//! The system's default output device, through cpal.

use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    BufferSize, ErrorKind, FromSample, I24, OutputCallbackInfo, SampleFormat, SizedSample,
    StreamConfig, SupportedStreamConfig, SupportedStreamConfigRange, U24,
};

use super::{OutputFormat, output_channels};
use crate::AudioError;
use crate::ring::RingReader;
use crate::shared::Shared;

/// Frames rendered per step when the device wants a sample format other than `f32`.
const CONVERT_FRAMES: usize = 1024;
/// The largest `f32` below 1.0. Converting exactly 1.0 to a 24-bit integer would overflow.
const MAX_SAMPLE: f32 = 1.0 - f32::EPSILON;

pub(crate) struct DeviceSink {
    device: cpal::Device,
    /// The configurations the device supports, limited to sample formats we can produce.
    configs: Vec<SupportedStreamConfigRange>,
    default: Option<SupportedStreamConfig>,
}

impl DeviceSink {
    pub fn open_default() -> Result<Self, AudioError> {
        let device = cpal::default_host()
            .default_output_device()
            .ok_or(AudioError::NoDevice)?;
        let configs = match device.supported_output_configs() {
            Ok(configs) => configs
                .filter(|config| format_rank(config.sample_format()) > 0)
                .collect(),
            Err(error) => {
                log::warn!("could not list the output device's formats: {error}");
                Vec::new()
            }
        };
        let default = device.default_output_config().ok();
        if configs.is_empty() && default.is_none() {
            return Err(AudioError::NoDevice);
        }
        Ok(Self {
            device,
            configs,
            default,
        })
    }

    pub fn format_for(&self, sample_rate: u32) -> OutputFormat {
        choose_format(&self.configs, self.default.as_ref(), sample_rate)
    }

    /// Builds a paused stream in `format` that plays what `reader` renders.
    pub fn build(
        &self,
        format: OutputFormat,
        mut reader: RingReader,
        shared: Arc<Shared>,
        id: u64,
        on_error: impl FnMut(u64, String) + Send + 'static,
    ) -> Result<cpal::Stream, AudioError> {
        let config = StreamConfig {
            channels: format.channels,
            sample_rate: format.sample_rate,
            buffer_size: BufferSize::Default,
        };
        let device = &self.device;
        let errors = error_callback(id, on_error);
        let stream = match format.sample_format {
            SampleFormat::F32 => device.build_output_stream(
                config,
                move |out: &mut [f32], info: &OutputCallbackInfo| {
                    record_latency(&shared, info);
                    reader.render(out);
                },
                errors,
                None,
            ),
            SampleFormat::F64 => build_converting::<f64>(device, config, reader, shared, errors),
            SampleFormat::I32 => build_converting::<i32>(device, config, reader, shared, errors),
            SampleFormat::I24 => build_converting::<I24>(device, config, reader, shared, errors),
            SampleFormat::I16 => build_converting::<i16>(device, config, reader, shared, errors),
            SampleFormat::I8 => build_converting::<i8>(device, config, reader, shared, errors),
            SampleFormat::I64 => build_converting::<i64>(device, config, reader, shared, errors),
            SampleFormat::U32 => build_converting::<u32>(device, config, reader, shared, errors),
            SampleFormat::U24 => build_converting::<U24>(device, config, reader, shared, errors),
            SampleFormat::U16 => build_converting::<u16>(device, config, reader, shared, errors),
            SampleFormat::U8 => build_converting::<u8>(device, config, reader, shared, errors),
            SampleFormat::U64 => build_converting::<u64>(device, config, reader, shared, errors),
            other => {
                return Err(AudioError::Output(format!(
                    "unsupported sample format {other}"
                )));
            }
        };
        stream.map_err(|error| AudioError::Output(error.to_string()))
    }
}

/// Builds a stream for a device that wants samples of type `T`: audio is rendered as `f32`
/// into a buffer allocated up front, then converted.
fn build_converting<T>(
    device: &cpal::Device,
    config: StreamConfig,
    mut reader: RingReader,
    shared: Arc<Shared>,
    errors: impl FnMut(cpal::Error) + Send + 'static,
) -> Result<cpal::Stream, cpal::Error>
where
    T: SizedSample + FromSample<f32>,
{
    let mut scratch = vec![0.0; CONVERT_FRAMES * reader.channels()];
    device.build_output_stream(
        config,
        move |out: &mut [T], info: &OutputCallbackInfo| {
            record_latency(&shared, info);
            for chunk in out.chunks_mut(scratch.len()) {
                let rendered = &mut scratch[..chunk.len()];
                reader.render(rendered);
                for (sample, &value) in chunk.iter_mut().zip(rendered.iter()) {
                    *sample = T::from_sample(value.clamp(-1.0, MAX_SAMPLE));
                }
            }
        },
        errors,
        None,
    )
}

fn record_latency(shared: &Shared, info: &OutputCallbackInfo) {
    let timestamp = info.timestamp();
    shared.set_latency(
        timestamp
            .playback
            .saturating_duration_since(timestamp.callback),
    );
}

/// Wraps `on_error` for cpal: problems the backend recovers from by itself are ignored, and
/// only the first real failure of a stream is reported.
fn error_callback(
    id: u64,
    mut on_error: impl FnMut(u64, String) + Send + 'static,
) -> impl FnMut(cpal::Error) + Send + 'static {
    let mut reported = false;
    move |error: cpal::Error| match error.kind() {
        ErrorKind::Xrun | ErrorKind::DeviceChanged | ErrorKind::RealtimeDenied => {}
        _ if !reported => {
            reported = true;
            on_error(id, error.to_string());
        }
        _ => {}
    }
}

pub(super) fn play(stream: &cpal::Stream) -> Result<(), AudioError> {
    stream
        .play()
        .map_err(|error| AudioError::Output(error.to_string()))
}

/// Pauses the stream. Returns false if the device cannot pause.
pub(super) fn pause(stream: &cpal::Stream) -> bool {
    match stream.pause() {
        Ok(()) => true,
        Err(error) => {
            log::debug!("output device keeps running while paused: {error}");
            false
        }
    }
}

/// Picks the stream format for audio at `sample_rate`: stereo (see [`output_channels`]),
/// the audio's own rate if the device supports it, else the device's default rate, else
/// the closest it supports; and the most precise sample format available at that rate.
fn choose_format(
    configs: &[SupportedStreamConfigRange],
    default: Option<&SupportedStreamConfig>,
    sample_rate: u32,
) -> OutputFormat {
    let channels = output_channels(|count| configs.iter().any(|c| c.channels() == count))
        .or(default.map(SupportedStreamConfig::channels))
        .unwrap_or(2);
    let candidates: Vec<_> = configs
        .iter()
        .filter(|config| config.channels() == channels)
        .collect();
    let default_rate = default.map(SupportedStreamConfig::sample_rate);

    let rate = [Some(sample_rate), default_rate]
        .into_iter()
        .flatten()
        .find(|&rate| candidates.iter().any(|config| config.contains_rate(rate)))
        .or_else(|| closest_rate(&candidates, sample_rate))
        .or(default_rate)
        .unwrap_or(sample_rate);

    let sample_format = candidates
        .iter()
        .filter(|config| config.contains_rate(rate))
        .map(|config| config.sample_format())
        .max_by_key(|&format| format_rank(format))
        .or(default.map(SupportedStreamConfig::sample_format))
        .unwrap_or(SampleFormat::F32);

    OutputFormat {
        sample_rate: rate,
        channels,
        sample_format,
    }
}

fn closest_rate(configs: &[&SupportedStreamConfigRange], rate: u32) -> Option<u32> {
    configs
        .iter()
        .map(|config| rate.clamp(config.min_sample_rate(), config.max_sample_rate()))
        .min_by_key(|candidate| candidate.abs_diff(rate))
}

/// Preference among sample formats, best first. `f32` is what the engine produces, so it
/// passes audio through untouched. 0 means the format cannot be produced (DSD).
fn format_rank(format: SampleFormat) -> u8 {
    match format {
        SampleFormat::F32 => 12,
        SampleFormat::F64 => 11,
        SampleFormat::I32 => 10,
        SampleFormat::I24 => 9,
        SampleFormat::U32 => 8,
        SampleFormat::U24 => 7,
        SampleFormat::I16 => 6,
        SampleFormat::U16 => 5,
        SampleFormat::I8 => 4,
        SampleFormat::U8 => 3,
        SampleFormat::I64 => 2,
        SampleFormat::U64 => 1,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cpal::Sample;

    fn range(channels: u16, rates: (u32, u32), format: SampleFormat) -> SupportedStreamConfigRange {
        SupportedStreamConfigRange::new(
            channels,
            rates.0,
            rates.1,
            cpal::SupportedBufferSize::Unknown,
            format,
        )
    }

    #[test]
    fn keeps_native_rate_when_supported() {
        let configs = [
            range(2, (8_000, 192_000), SampleFormat::I16),
            range(2, (8_000, 192_000), SampleFormat::F32),
            range(6, (8_000, 192_000), SampleFormat::F32),
        ];
        let format = choose_format(&configs, None, 96_000);
        assert_eq!(format.sample_rate, 96_000);
        assert_eq!(format.channels, 2);
        assert_eq!(format.sample_format, SampleFormat::F32);
    }

    #[test]
    fn falls_back_to_the_default_rate() {
        let configs = [range(2, (44_100, 48_000), SampleFormat::I32)];
        let default = configs[0].try_with_sample_rate(48_000);
        let format = choose_format(&configs, default.as_ref(), 96_000);
        assert_eq!(format.sample_rate, 48_000);
        assert_eq!(format.sample_format, SampleFormat::I32);
    }

    #[test]
    fn uses_the_closest_rate_without_a_default() {
        let configs = [
            range(2, (32_000, 32_000), SampleFormat::I16),
            range(2, (88_200, 88_200), SampleFormat::I16),
        ];
        assert_eq!(choose_format(&configs, None, 96_000).sample_rate, 88_200);
    }

    #[test]
    fn uses_mono_on_mono_only_devices() {
        let configs = [range(1, (48_000, 48_000), SampleFormat::I16)];
        assert_eq!(choose_format(&configs, None, 48_000).channels, 1);
    }

    #[test]
    fn converts_full_scale_without_overflow() {
        assert_eq!(I24::from_sample(MAX_SAMPLE).inner(), 8_388_607);
        assert_eq!(i16::from_sample(MAX_SAMPLE), i16::MAX);
        assert_eq!(i16::from_sample(-1.0_f32), i16::MIN);
    }
}
