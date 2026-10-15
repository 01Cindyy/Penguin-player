//! A guard against ID3v2 chapter frames nested deeply enough to overflow the stack.
//!
//! lofty parses a chapter frame (`CHAP` or `CTOC`) by recursively parsing the frames
//! inside it, with tens of kilobytes of stack per level. A crafted file of a few
//! kilobytes can nest chapters deep enough to overflow a thread's stack, which aborts
//! the whole app: unlike a panic, an overflow can't be caught.
//!
//! lofty can only parse a frame after reading it, so counting the chapter frame IDs in
//! everything it reads, and failing the read once there are too many, bounds how deep
//! it can go. Tag reading also runs on threads with a large stack (see
//! [`TAG_READER_STACK_BYTES`]).
//!
//! Known limit: chapters inside a *compressed* ID3v2 frame are decompressed by lofty
//! and never pass through this counter. Compressed frames are practically unused in
//! real files, and lofty 0.25 doesn't build without its compression support, so the
//! large reader stack is the only guard there.

use std::io::{self, Read, Seek, SeekFrom};

/// The most chapter frames a file's tags are read with. Podcasts and audiobooks have
/// tens to a few hundred; a file with more is read without its tags.
pub(crate) const MAX_CHAPTER_FRAMES: usize = 256;

/// Stack size for threads that read tags: enough for lofty to parse
/// [`MAX_CHAPTER_FRAMES`] nested chapters (up to about 80 KiB each in an optimised build)
/// with room to spare. Only address space is reserved; memory is used only as deep as
/// parsing actually goes.
pub(crate) const TAG_READER_STACK_BYTES: usize = 64 * 1024 * 1024;

const CHAPTER_FRAME_IDS: [u32; 2] = [u32::from_be_bytes(*b"CHAP"), u32::from_be_bytes(*b"CTOC")];

/// Passes reads through while counting chapter frame IDs, and fails every read once
/// more than its limit (normally [`MAX_CHAPTER_FRAMES`]) have gone past.
pub(crate) struct ChapterLimit<R> {
    inner: R,
    limit: usize,
    /// The last four bytes read, so IDs split across two reads are counted too.
    recent: u32,
    chapter_frames: usize,
}

impl<R> ChapterLimit<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            limit: MAX_CHAPTER_FRAMES,
            recent: 0,
            chapter_frames: 0,
        }
    }

    /// No limit, for reads that skip the tags: lofty seeks past them without parsing.
    pub fn unlimited(inner: R) -> Self {
        Self {
            limit: usize::MAX,
            ..Self::new(inner)
        }
    }

    fn count_chapter_frames(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.recent = (self.recent << 8) | u32::from(byte);
            if CHAPTER_FRAME_IDS.contains(&self.recent) {
                self.chapter_frames += 1;
            }
        }
    }
}

impl<R: Read> Read for ChapterLimit<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buf)?;
        self.count_chapter_frames(&buf[..read]);
        if self.chapter_frames > self.limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("more than {MAX_CHAPTER_FRAMES} ID3v2 chapter frames"),
            ));
        }
        Ok(read)
    }
}

/// Seeking keeps `recent` as it is. Bytes on either side of a jump may then look like an
/// ID that isn't really there, but counting too many is safe; missing one is not.
impl<R: Seek> Seek for ChapterLimit<R> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.inner.seek(position)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn read_in_chunks(data: &[u8], chunk: usize) -> io::Result<usize> {
        let mut reader = ChapterLimit::new(Cursor::new(data));
        let mut buf = vec![0; chunk];
        while reader.read(&mut buf)? > 0 {}
        Ok(reader.chapter_frames)
    }

    #[test]
    fn counts_ids_split_across_reads() {
        let data = b"..CHAP..CTOC..CHA".repeat(3);
        for chunk in 1..8 {
            assert_eq!(read_in_chunks(&data, chunk).unwrap(), 6, "chunk {chunk}");
        }
    }

    #[test]
    fn fails_once_there_are_too_many() {
        let at_limit = b"CHAP".repeat(MAX_CHAPTER_FRAMES);
        assert_eq!(read_in_chunks(&at_limit, 4096).unwrap(), MAX_CHAPTER_FRAMES);
        let over_limit = b"CTOC".repeat(MAX_CHAPTER_FRAMES + 1);
        let error = read_in_chunks(&over_limit, 4096).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}
