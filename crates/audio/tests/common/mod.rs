//! Helpers shared by the integration tests.

// Each test binary uses a different subset of these helpers.
#![allow(dead_code)]

use std::path::Path;

/// Writes a 16-bit PCM WAV file whose frame `n` holds `sample(n)` on every channel.
pub fn write_wav(path: &Path, rate: u32, channels: u16, frames: u32, sample: impl Fn(u32) -> i16) {
    let data_len = frames * u32::from(channels) * 2;
    let mut bytes = Vec::with_capacity(44 + data_len as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&channels.to_le_bytes());
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(rate * u32::from(channels) * 2).to_le_bytes());
    bytes.extend_from_slice(&(channels * 2).to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for frame in 0..frames {
        let value = sample(frame);
        for _ in 0..channels {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    std::fs::write(path, bytes).expect("write wav");
}

/// Writes an M4A file of 16-bit stereo PCM (`sowt`) whose frame `n` holds `sample(n)` on
/// every channel. Only the boxes Symphonia's MP4 reader requires are written.
pub fn write_m4a(path: &Path, rate: u32, frames: u32, sample: impl Fn(u32) -> i16) {
    let pcm: Vec<u8> = (0..frames)
        .flat_map(|frame| {
            let [low, high] = sample(frame).to_le_bytes();
            [low, high, low, high]
        })
        .collect();
    // Every full box starts with a zero version/flags word. The sample entry holds reserved
    // words, data reference 1, 2 channels of 16 bits and the rate in 16.16 fixed point.
    let sample_entry = mp4_box(
        b"sowt",
        &[&words(&[0, 1, 0, 0, 0x0002_0010, 0, rate << 16])],
    );
    let moov = |data_offset: u32| {
        let stbl = mp4_box(
            b"stbl",
            &[
                &mp4_box(b"stsd", &[&words(&[0, 1]), &sample_entry]),
                &mp4_box(b"stts", &[&words(&[0, 1, frames, 1])]),
                &mp4_box(b"stsc", &[&words(&[0, 1, 1, frames, 1])]),
                &mp4_box(b"stsz", &[&words(&[0, 4, frames])]),
                &mp4_box(b"stco", &[&words(&[0, 1, data_offset])]),
            ],
        );
        let sound = u32::from_be_bytes(*b"soun");
        let mdia = mp4_box(
            b"mdia",
            &[
                &mp4_box(b"mdhd", &[&words(&[0, 0, 0, rate, frames, 0])]),
                &mp4_box(b"hdlr", &[&words(&[0, 0, sound, 0, 0, 0])]),
                &mp4_box(b"minf", &[&stbl]),
            ],
        );
        let tkhd = mp4_box(b"tkhd", &[&words(&[0, 0, 0, 1, 0, frames, 0, 0, 0, 0])]);
        let mvhd = mp4_box(b"mvhd", &[&words(&[0, 0, 0, rate, frames, 0x0001_0000, 0])]);
        mp4_box(b"moov", &[&mvhd, &mp4_box(b"trak", &[&tkhd, &mdia])])
    };
    let ftyp = mp4_box(b"ftyp", &[b"M4A \0\0\0\0M4A isom"]);
    // The sample table points at the audio in the `mdat` box after `moov`.
    let data_offset = u32::try_from(ftyp.len() + moov(0).len() + 8).expect("small file");
    let file = [ftyp, moov(data_offset), mp4_box(b"mdat", &[&pcm])].concat();
    std::fs::write(path, file).expect("write m4a");
}

/// Writes a Matroska file of 16-bit stereo PCM whose frame `n` holds `sample(n)` on every
/// channel, in 10 ms blocks and one cluster per second. With `cues` a cue point is written
/// for every cluster.
pub fn write_mka(path: &Path, rate: u32, frames: u32, cues: bool, sample: impl Fn(u32) -> i16) {
    const BLOCK_MS: u32 = 10;
    const CLUSTER_MS: u32 = 1000;
    let block_frames = rate * BLOCK_MS / 1000;
    let clusters: Vec<Vec<u8>> = (0..frames)
        .step_by((rate * CLUSTER_MS / 1000) as usize)
        .map(|first| {
            let cluster_ms = first * 1000 / rate;
            let end = (first + rate * CLUSTER_MS / 1000).min(frames);
            let mut body = ebml(0xE7, &uint(u64::from(cluster_ms)));
            for block in (first..end).step_by(block_frames as usize) {
                let relative_ms = i16::try_from(block * 1000 / rate - cluster_ms).expect("short");
                let mut data = vec![0x81];
                data.extend_from_slice(&relative_ms.to_be_bytes());
                data.push(0x80);
                for frame in block..(block + block_frames).min(end) {
                    let [low, high] = sample(frame).to_le_bytes();
                    data.extend_from_slice(&[low, high, low, high]);
                }
                body.extend(ebml(0xA3, &data));
            }
            ebml(0x1F43_B675, &body)
        })
        .collect();

    let header = ebml(0x1A45_DFA3, &ebml(0x4282, b"matroska"));
    let duration_ms = f64::from(frames) * 1000.0 / f64::from(rate);
    let info = ebml(
        0x1549_A966,
        &[
            ebml(0x2A_D7B1, &uint(1_000_000)),
            ebml(0x4489, &duration_ms.to_be_bytes()),
            ebml(0x4D80, b"mp-audio tests"),
            ebml(0x5741, b"mp-audio tests"),
        ]
        .concat(),
    );
    let audio = [
        ebml(0xB5, &f64::from(rate).to_be_bytes()),
        ebml(0x9F, &uint(2)),
        ebml(0x6264, &uint(16)),
    ]
    .concat();
    let track = [
        ebml(0xD7, &uint(1)),
        ebml(0x73C5, &uint(1)),
        ebml(0x83, &uint(2)),
        ebml(0x86, b"A_PCM/INT/LIT"),
        ebml(0xE1, &audio),
    ]
    .concat();
    let tracks = ebml(0x1654_AE6B, &ebml(0xAE, &track));

    // Cue positions count from the start of the segment's body. Cues come before the
    // clusters, and fixed-width values keep their size independent of the positions.
    let cue_points = |first_cluster: usize| {
        let mut position = first_cluster;
        let points: Vec<u8> = clusters
            .iter()
            .enumerate()
            .flat_map(|(index, cluster)| {
                let positions = [ebml(0xF7, &uint(1)), ebml(0xF1, &uint(position as u64))];
                position += cluster.len();
                let point = [
                    ebml(0xB3, &uint(index as u64 * u64::from(CLUSTER_MS))),
                    ebml(0xB7, &positions.concat()),
                ];
                ebml(0xBB, &point.concat())
            })
            .collect();
        ebml(0x1C53_BB6B, &points)
    };
    let mut segment = [info, tracks].concat();
    if cues {
        let cues_len = cue_points(0).len();
        segment.extend(cue_points(segment.len() + cues_len));
    }
    segment.extend(clusters.concat());
    let file = [header, ebml(0x1853_8067, &segment)].concat();
    std::fs::write(path, file).expect("write mka");
}

/// An EBML element. Sizes are always written as 8-byte variable-length integers.
fn ebml(id: u32, body: &[u8]) -> Vec<u8> {
    let id_bytes = id.to_be_bytes();
    let id_start = id_bytes.iter().position(|&byte| byte != 0).unwrap_or(3);
    let mut size = (body.len() as u64).to_be_bytes();
    size[0] = 0x01;
    [&id_bytes[id_start..], &size, body].concat()
}

/// A fixed-width unsigned EBML integer.
fn uint(value: u64) -> [u8; 8] {
    value.to_be_bytes()
}

fn mp4_box(kind: &[u8; 4], parts: &[&[u8]]) -> Vec<u8> {
    let body = parts.concat();
    let size = u32::try_from(body.len() + 8).expect("small box");
    [&size.to_be_bytes(), kind.as_slice(), &body].concat()
}

/// Big-endian 32-bit words.
fn words(values: &[u32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_be_bytes())
        .collect()
}

/// A quiet 440 Hz tone at `rate`.
pub fn tone(rate: u32) -> impl Fn(u32) -> i16 {
    move |frame| {
        let phase = frame as f32 * 440.0 * std::f32::consts::TAU / rate as f32;
        (phase.sin() * 3000.0) as i16
    }
}
