//! Gapless information for MP4 (M4A) files.
//!
//! Symphonia's MP4 reader does not apply edit lists yet, so AAC encoder delay ("priming")
//! and end padding would otherwise be played. This reads the track's edit list directly.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// `moov` boxes larger than this are not read (they would hold far more than track tables).
const MAX_MOOV_SIZE: u64 = 64 * 1024 * 1024;

/// The playable part of a track, in media timescale units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EditList {
    /// Frames of encoder delay at the start.
    pub delay: u64,
    /// Number of playable frames after the delay, if the edit gives one.
    pub length: Option<u64>,
}

/// Reads the edit list of track `track_id`. Returns `None` if the file has no usable one.
pub(crate) fn read_edit_list(path: &Path, track_id: u32, media_timescale: u32) -> Option<EditList> {
    let moov = read_moov(&mut File::open(path).ok()?)?;
    let movie_timescale = children(&moov)
        .find(|(kind, _)| kind == b"mvhd")
        .and_then(|(_, mvhd)| movie_timescale(mvhd))?;

    children(&moov)
        .filter(|(kind, _)| kind == b"trak")
        .find(|(_, trak)| {
            children(trak)
                .find(|(kind, _)| kind == b"tkhd")
                .and_then(|(_, tkhd)| tkhd_track_id(tkhd))
                == Some(track_id)
        })
        .and_then(|(_, trak)| children(trak).find(|(kind, _)| kind == b"edts"))
        .and_then(|(_, edts)| children(edts).find(|(kind, _)| kind == b"elst"))
        .and_then(|(_, elst)| parse_elst(elst, movie_timescale, media_timescale))
}

/// Finds the top-level `moov` box (it may come before or after the media data).
fn read_moov(file: &mut File) -> Option<Vec<u8>> {
    let file_len = file.metadata().ok()?.len();
    let mut offset = 0;
    while offset + 8 <= file_len {
        file.seek(SeekFrom::Start(offset)).ok()?;
        let mut header = [0; 16];
        file.read_exact(&mut header[..8]).ok()?;
        let mut size = u64::from(u32::from_be_bytes(header[..4].try_into().ok()?));
        let mut header_len = 8;
        if size == 1 {
            file.read_exact(&mut header[8..]).ok()?;
            size = u64::from_be_bytes(header[8..].try_into().ok()?);
            header_len = 16;
        } else if size == 0 {
            size = file_len - offset;
        }
        if size < header_len {
            return None;
        }
        if &header[4..8] == b"moov" {
            let body_len = size - header_len;
            if body_len > MAX_MOOV_SIZE {
                return None;
            }
            let mut body = vec![0; usize::try_from(body_len).ok()?];
            file.read_exact(&mut body).ok()?;
            return Some(body);
        }
        offset = offset.checked_add(size)?;
    }
    None
}

/// Iterates over the boxes directly inside `data` as `(type, body)` pairs.
fn children(mut data: &[u8]) -> impl Iterator<Item = ([u8; 4], &[u8])> {
    std::iter::from_fn(move || {
        let size = usize::try_from(read_u32(data, 0)?).ok()?;
        let kind: [u8; 4] = data.get(4..8)?.try_into().ok()?;
        let (header_len, size) = match size {
            0 => (8, data.len()),
            1 => (16, usize::try_from(read_u64(data, 8)?).ok()?),
            size => (8, size),
        };
        let body = data.get(header_len..size)?;
        data = &data[size..];
        Some((kind, body))
    })
}

fn movie_timescale(mvhd: &[u8]) -> Option<u32> {
    // version + flags, then creation and modification times (32 or 64 bit), then timescale.
    let offset = if *mvhd.first()? == 1 { 20 } else { 12 };
    read_u32(mvhd, offset).filter(|&scale| scale > 0)
}

fn tkhd_track_id(tkhd: &[u8]) -> Option<u32> {
    let offset = if *tkhd.first()? == 1 { 20 } else { 12 };
    read_u32(tkhd, offset)
}

/// Uses the first edit that plays media. Empty edits (media time -1) only add silence
/// before the track, which a music player can ignore.
fn parse_elst(elst: &[u8], movie_timescale: u32, media_timescale: u32) -> Option<EditList> {
    let wide = *elst.first()? == 1;
    let entries = usize::try_from(read_u32(elst, 4)?).ok()?;
    let entry_len = if wide { 20 } else { 12 };
    elst.get(8..)?
        .chunks_exact(entry_len)
        .take(entries)
        .find_map(|entry| {
            let (duration, media_time) = if wide {
                (read_u64(entry, 0)?, read_u64(entry, 8)? as i64)
            } else {
                (
                    u64::from(read_u32(entry, 0)?),
                    i64::from(read_u32(entry, 4)? as i32),
                )
            };
            let delay = u64::try_from(media_time).ok()?;
            let length = (duration > 0).then(|| {
                u128::from(duration) * u128::from(media_timescale) / u128::from(movie_timescale)
            });
            Some(EditList {
                delay,
                length: length.and_then(|length| u64::try_from(length).ok()),
            })
        })
}

fn read_u32(data: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        data.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn read_u64(data: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_be_bytes(
        data.get(offset..offset + 8)?.try_into().ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::time::{Duration, Instant};

    use super::*;

    fn boxed(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut data = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        data.extend_from_slice(kind);
        data.extend_from_slice(body);
        data
    }

    #[test]
    fn reads_priming_and_length() {
        let mut elst = vec![0, 0, 0, 0, 0, 0, 0, 1];
        elst.extend_from_slice(&20_000u32.to_be_bytes());
        elst.extend_from_slice(&1024u32.to_be_bytes());
        elst.extend_from_slice(&0x0001_0000u32.to_be_bytes());
        let edit = parse_elst(&elst, 1000, 44_100).expect("one edit");
        assert_eq!(
            edit,
            EditList {
                delay: 1024,
                length: Some(882_000)
            }
        );
    }

    #[test]
    fn stops_at_the_entries_the_box_holds() {
        // One empty edit, but a corrupt count claiming four billion entries.
        let mut elst = vec![0, 0, 0, 0];
        elst.extend_from_slice(&u32::MAX.to_be_bytes());
        elst.extend_from_slice(&[0, 0, 0, 10, 0xff, 0xff, 0xff, 0xff, 0, 1, 0, 0]);
        let started = Instant::now();
        assert_eq!(parse_elst(&elst, 1000, 44_100), None);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn survives_a_box_size_past_the_end_of_the_address_space() {
        let mut huge = vec![0, 0, 0, 1];
        huge.extend_from_slice(b"free");
        huge.extend_from_slice(&(u64::MAX - 4).to_be_bytes());
        let mut file = tempfile::tempfile().expect("temp file");
        file.write_all(&[boxed(b"ftyp", b"M4A "), huge].concat())
            .expect("write");
        assert_eq!(read_moov(&mut file), None);
    }

    #[test]
    fn walks_nested_boxes() {
        let inner = boxed(b"elst", &[1, 2, 3]);
        let outer = [boxed(b"free", &[]), boxed(b"edts", &inner)].concat();
        let edts = children(&outer)
            .find(|(kind, _)| kind == b"edts")
            .unwrap()
            .1;
        assert_eq!(children(edts).next(), Some((*b"elst", &[1u8, 2, 3][..])));
    }
}
