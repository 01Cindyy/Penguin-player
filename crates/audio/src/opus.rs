//! Opus decoding through libopus.
//!
//! Symphonia demuxes Opus (Ogg, Matroska/WebM) but has no Opus decoder, so this adapter
//! implements Symphonia's `AudioDecoder` on top of the `opus` crate and is registered next
//! to Symphonia's own codecs.

use std::sync::{Mutex, PoisonError};

use symphonia::core::audio::{
    AudioBuffer, AudioMut, AudioSpec, Channels, GenericAudioBufferRef, Position,
};
use symphonia::core::codecs::CodecInfo;
use symphonia::core::codecs::audio::well_known::CODEC_ID_OPUS;
use symphonia::core::codecs::audio::{
    AudioCodecParameters, AudioDecoder, AudioDecoderOptions, FinalizeResult,
};
use symphonia::core::codecs::registry::{RegisterableAudioDecoder, SupportedAudioCodec};
use symphonia::core::errors::{Error, Result};
use symphonia::core::packet::PacketRef;

/// Opus always decodes at 48 kHz.
const SAMPLE_RATE: u32 = 48_000;
/// The longest Opus packet holds 120 ms of audio.
const MAX_PACKET_FRAMES: usize = 5_760;

const SUPPORTED_CODECS: &[SupportedAudioCodec] = &[SupportedAudioCodec {
    id: CODEC_ID_OPUS,
    info: CodecInfo {
        short_name: "opus",
        long_name: "Opus (libopus)",
        profiles: &[],
    },
}];

pub(crate) struct OpusDecoder {
    params: AudioCodecParameters,
    // `AudioDecoder` must be `Sync`, but libopus state is only `Send`. It is only ever used
    // through `&mut self` via `Mutex::get_mut`, which never locks, so this costs nothing.
    decoder: Mutex<opus::MSDecoder>,
    /// For each channel libopus outputs, the plane it belongs to in `buffer`.
    plane_of_channel: Vec<usize>,
    pcm: Vec<f32>,
    buffer: AudioBuffer<f32>,
    gapless: bool,
}

impl OpusDecoder {
    fn try_new(params: &AudioCodecParameters, options: &AudioDecoderOptions) -> Result<Self> {
        let head = params
            .extra_data
            .as_deref()
            .and_then(OpusHead::parse)
            .or_else(|| OpusHead::implied(params.channels.as_ref()?.count()))
            .ok_or(Error::Unsupported("opus: missing or invalid header"))?;

        let mut decoder = opus::MSDecoder::new(
            SAMPLE_RATE,
            head.streams,
            head.coupled_streams,
            &head.mapping,
        )
        .map_err(|_| Error::Unsupported("opus: unsupported channel mapping"))?;
        if head.output_gain != 0 {
            decoder
                .set_gain(i32::from(head.output_gain))
                .map_err(|_| Error::DecodeError("opus: invalid output gain"))?;
        }

        let channel_count = head.mapping.len();
        let (layout, plane_of_channel) = channel_layout(channel_count, head.mapping_family);
        let mut params = params.clone();
        params
            .with_sample_rate(SAMPLE_RATE)
            .with_channels(layout.clone());

        Ok(Self {
            params,
            decoder: Mutex::new(decoder),
            plane_of_channel,
            pcm: vec![0.0; MAX_PACKET_FRAMES * channel_count],
            buffer: AudioBuffer::new(AudioSpec::new(SAMPLE_RATE, layout), MAX_PACKET_FRAMES),
            gapless: options.gapless,
        })
    }

    fn decoder(&mut self) -> &mut opus::MSDecoder {
        self.decoder
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

impl AudioDecoder for OpusDecoder {
    fn reset(&mut self) {
        // Resetting only fails for an invalid decoder handle, which `try_new` rules out.
        let _ = self.decoder().reset_state();
    }

    fn codec_info(&self) -> &CodecInfo {
        &SUPPORTED_CODECS[0].info
    }

    fn codec_params(&self) -> &AudioCodecParameters {
        &self.params
    }

    fn decode_ref(&mut self, packet: &PacketRef<'_>) -> Result<GenericAudioBufferRef<'_>> {
        self.buffer.clear();
        let mut pcm = std::mem::take(&mut self.pcm);
        let decoded = self.decoder().decode_float(packet.data, &mut pcm, false);
        self.pcm = pcm;
        let frames = decoded.map_err(|_| Error::DecodeError("opus: invalid packet"))?;

        self.buffer.render_uninit(Some(frames));
        let channels = self.plane_of_channel.len();
        let interleaved = &self.pcm[..frames * channels];
        for (channel, &plane) in self.plane_of_channel.iter().enumerate() {
            if let Some(plane) = self.buffer.plane_mut(plane) {
                for (sample, frame) in plane.iter_mut().zip(interleaved.chunks_exact(channels)) {
                    *sample = frame[channel];
                }
            }
        }

        if self.gapless {
            let trim_start = usize::try_from(packet.trim_start.get()).unwrap_or(usize::MAX);
            let trim_end = usize::try_from(packet.trim_end.get()).unwrap_or(usize::MAX);
            self.buffer.trim(trim_start, trim_end);
        }
        Ok((&self.buffer).into())
    }

    fn finalize(&mut self) -> FinalizeResult {
        FinalizeResult::default()
    }

    fn last_decoded(&self) -> GenericAudioBufferRef<'_> {
        (&self.buffer).into()
    }
}

impl RegisterableAudioDecoder for OpusDecoder {
    fn try_registry_new(
        params: &AudioCodecParameters,
        options: &AudioDecoderOptions,
    ) -> Result<Box<dyn AudioDecoder>> {
        Ok(Box::new(Self::try_new(params, options)?))
    }

    fn supported_codecs() -> &'static [SupportedAudioCodec] {
        SUPPORTED_CODECS
    }
}

/// The fields of the Opus identification header ("OpusHead", RFC 7845) the decoder needs.
struct OpusHead {
    /// Q7.8 dB gain to apply to the decoded output.
    output_gain: i16,
    mapping_family: u8,
    streams: u8,
    coupled_streams: u8,
    /// Decoded channel → stream channel; its length is the output channel count.
    mapping: Vec<u8>,
}

impl OpusHead {
    fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < 19 || !data.starts_with(b"OpusHead") {
            return None;
        }
        let channels = data[9];
        let output_gain = i16::from_le_bytes([data[16], data[17]]);
        let mapping_family = data[18];
        if mapping_family == 0 {
            return Self::implied(usize::from(channels)).map(|head| Self {
                output_gain,
                ..head
            });
        }
        let table = data.get(19..21 + usize::from(channels))?;
        Some(Self {
            output_gain,
            mapping_family,
            streams: table[0],
            coupled_streams: table[1],
            mapping: table[2..].to_vec(),
        })
    }

    /// The single-stream mono or stereo layout used when the header has no mapping table.
    fn implied(channels: usize) -> Option<Self> {
        let (coupled_streams, mapping) = match channels {
            1 => (0, vec![0]),
            2 => (1, vec![0, 1]),
            _ => return None,
        };
        Some(Self {
            output_gain: 0,
            mapping_family: 0,
            streams: 1,
            coupled_streams,
            mapping,
        })
    }
}

/// Speaker layout for Opus's channel order (the Vorbis order, RFC 7845 §5.1.1.2), and the
/// buffer plane of each decoded channel. Symphonia buffers keep positioned channels in
/// ascending position order, which differs from the Vorbis order for surround layouts.
fn channel_layout(channels: usize, mapping_family: u8) -> (Channels, Vec<usize>) {
    let order = if mapping_family <= 1 {
        vorbis_channel_order(channels)
    } else {
        None
    };
    let Some(order) = order else {
        let count = u16::try_from(channels).unwrap_or(u16::MAX);
        return (Channels::Discrete(count), (0..channels).collect());
    };
    let mask = order
        .iter()
        .fold(Position::empty(), |mask, &position| mask | position);
    let planes = order
        .iter()
        .map(|position| (mask.bits() & (position.bits() - 1)).count_ones() as usize)
        .collect();
    (Channels::Positioned(mask), planes)
}

fn vorbis_channel_order(channels: usize) -> Option<&'static [Position]> {
    const FL: Position = Position::FRONT_LEFT;
    const FR: Position = Position::FRONT_RIGHT;
    const FC: Position = Position::FRONT_CENTER;
    const LFE: Position = Position::LFE1;
    const RL: Position = Position::REAR_LEFT;
    const RR: Position = Position::REAR_RIGHT;
    const RC: Position = Position::REAR_CENTER;
    const SL: Position = Position::SIDE_LEFT;
    const SR: Position = Position::SIDE_RIGHT;
    let order: &'static [Position] = match channels {
        1 => &[FC],
        2 => &[FL, FR],
        3 => &[FL, FC, FR],
        4 => &[FL, FR, RL, RR],
        5 => &[FL, FC, FR, RL, RR],
        6 => &[FL, FC, FR, RL, RR, LFE],
        7 => &[FL, FC, FR, SL, SR, RC, LFE],
        8 => &[FL, FC, FR, SL, SR, RL, RR, LFE],
        _ => return None,
    };
    Some(order)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stereo_header() {
        let mut head = b"OpusHead".to_vec();
        head.extend_from_slice(&[1, 2, 0x38, 0x01, 0x80, 0xbb, 0, 0, 0x00, 0x01, 0]);
        let parsed = OpusHead::parse(&head).expect("valid header");
        assert_eq!(parsed.output_gain, 256);
        assert_eq!((parsed.streams, parsed.coupled_streams), (1, 1));
        assert_eq!(parsed.mapping, vec![0, 1]);
    }

    #[test]
    fn surround_channels_map_to_canonical_planes() {
        let (layout, planes) = channel_layout(6, 1);
        assert_eq!(layout.count(), 6);
        // Vorbis order FL FC FR RL RR LFE → canonical FL FR FC LFE RL RR.
        assert_eq!(planes, vec![0, 2, 1, 4, 5, 3]);
    }
}
