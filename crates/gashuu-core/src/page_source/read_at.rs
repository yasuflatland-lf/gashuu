//! Positional byte sources and independent cursors for archive readers.

#[cfg(not(target_arch = "wasm32"))]
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;
use std::sync::Arc;

/// Positional, thread-safe random access to an immutable byte source.
pub trait ReadAt: Send + Sync {
    /// Total length in bytes.
    fn len(&self) -> u64;

    /// Whether the source contains zero bytes.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Read up to `buf.len()` bytes starting at `offset`.
    ///
    /// Returns `Ok(0)` at or past the end. Callers that need a full buffer must
    /// loop on short reads.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize>;
}

/// Cheaply cloneable `Read + Seek` view over a shared [`ReadAt`].
///
/// A clone starts at the same position and advances independently.
#[derive(Clone)]
pub struct ReadAtCursor {
    src: Arc<dyn ReadAt>,
    pos: u64,
}

impl ReadAtCursor {
    /// Create a cursor positioned at the start of the source.
    pub fn new(src: Arc<dyn ReadAt>) -> Self {
        Self { src, pos: 0 }
    }
}

impl Read for ReadAtCursor {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.src.read_at(self.pos, buf)?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for ReadAtCursor {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let pos = match pos {
            SeekFrom::Start(pos) => Some(pos),
            SeekFrom::End(offset) => self.src.len().checked_add_signed(offset),
            SeekFrom::Current(offset) => self.pos.checked_add_signed(offset),
        };
        self.pos = pos.ok_or(io::ErrorKind::InvalidInput)?;
        Ok(self.pos)
    }
}

impl ReadAt for Arc<[u8]> {
    fn len(&self) -> u64 {
        self.as_ref().len() as u64
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        if offset >= self.len() {
            return Ok(0);
        }
        let tail = &self[offset as usize..];
        let n = buf.len().min(tail.len());
        buf[..n].copy_from_slice(&tail[..n]);
        Ok(n)
    }
}

/// A desktop file held open for positional reads.
#[cfg(not(target_arch = "wasm32"))]
pub struct FileReadAt {
    file: File,
    len: u64,
}

#[cfg(not(target_arch = "wasm32"))]
impl FileReadAt {
    /// Open the file and record its length.
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        Ok(Self { file, len })
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl ReadAt for FileReadAt {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let remaining = self.len.saturating_sub(offset);
        let limit = buf
            .len()
            .min(usize::try_from(remaining).unwrap_or(usize::MAX));
        if limit == 0 {
            return Ok(0);
        }
        let buf = &mut buf[..limit];
        #[cfg(unix)]
        {
            std::os::unix::fs::FileExt::read_at(&self.file, buf, offset)
        }
        #[cfg(windows)]
        {
            // Each request supplies its offset, so the file's cursor is never consulted.
            std::os::windows::fs::FileExt::seek_read(&self.file, buf, offset)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cursor() -> ReadAtCursor {
        ReadAtCursor::new(Arc::new(Arc::<[u8]>::from(&b"abcdef"[..])))
    }

    #[test]
    fn cursor_reads_sequentially_and_stops_at_eof() {
        let mut cursor = cursor();
        let mut head = [0; 2];
        cursor.read_exact(&mut head).unwrap();
        assert_eq!(&head, b"ab");

        let mut tail = Vec::new();
        assert_eq!(cursor.read_to_end(&mut tail).unwrap(), 4);
        assert_eq!(tail.as_slice(), b"cdef");
        assert_eq!(cursor.read(&mut head).unwrap(), 0);
    }

    #[test]
    fn cursor_seek_start_end_current() {
        let mut cursor = cursor();
        let mut byte = [0; 1];

        assert_eq!(cursor.seek(SeekFrom::Start(1)).unwrap(), 1);
        cursor.read_exact(&mut byte).unwrap();
        assert_eq!(&byte, b"b");

        assert_eq!(cursor.seek(SeekFrom::Current(1)).unwrap(), 3);
        cursor.read_exact(&mut byte).unwrap();
        assert_eq!(&byte, b"d");

        assert_eq!(cursor.seek(SeekFrom::End(-1)).unwrap(), 5);
        cursor.read_exact(&mut byte).unwrap();
        assert_eq!(&byte, b"f");

        assert_eq!(cursor.seek(SeekFrom::End(1)).unwrap(), 7);
        assert_eq!(cursor.read(&mut byte).unwrap(), 0);
    }

    #[test]
    fn cursor_seek_before_start_is_invalid_input() {
        let mut cursor = cursor();
        for seek in [SeekFrom::Current(-1), SeekFrom::End(-7)] {
            assert_eq!(
                cursor.seek(seek).unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
            assert_eq!(cursor.stream_position().unwrap(), 0);
        }

        cursor.seek(SeekFrom::Start(u64::MAX)).unwrap();
        assert_eq!(
            cursor.seek(SeekFrom::Current(1)).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(cursor.stream_position().unwrap(), u64::MAX);
    }

    #[test]
    fn cursor_clone_has_independent_position() {
        let mut original = cursor();
        original.seek(SeekFrom::Start(2)).unwrap();
        let mut clone = original.clone();

        let mut pair = [0; 2];
        clone.read_exact(&mut pair).unwrap();
        assert_eq!(&pair, b"cd");
        assert_eq!(clone.stream_position().unwrap(), 4);
        assert_eq!(original.stream_position().unwrap(), 2);

        original.rewind().unwrap();
        clone.read_exact(&mut pair).unwrap();
        assert_eq!(&pair, b"ef");
    }

    #[test]
    fn arc_slice_read_at_offsets_short_tail_and_past_end() {
        let bytes = Arc::<[u8]>::from(&b"abcdef"[..]);
        assert_eq!(ReadAt::len(&bytes), 6);
        assert!(!ReadAt::is_empty(&bytes));

        let mut buf = [0; 3];
        assert_eq!(bytes.read_at(2, &mut buf).unwrap(), 3);
        assert_eq!(&buf, b"cde");
        assert_eq!(bytes.read_at(4, &mut buf).unwrap(), 2);
        assert_eq!(&buf[..2], b"ef");
        assert_eq!(bytes.read_at(6, &mut buf).unwrap(), 0);
        assert_eq!(bytes.read_at(7, &mut buf).unwrap(), 0);
        assert_eq!(bytes.read_at(u64::MAX, &mut buf).unwrap(), 0);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn file_read_at_concurrent_positional_reads() {
        use std::io::Write;
        use std::sync::Barrier;

        const FILE_LEN: usize = 1024 * 1024;
        let expected: Vec<u8> = (0..FILE_LEN)
            .map(|i| (i ^ (i >> 8) ^ (i >> 16)) as u8)
            .collect();
        let mut temp = tempfile::NamedTempFile::new().unwrap();
        temp.write_all(&expected).unwrap();
        let source = FileReadAt::open(temp.path()).unwrap();
        assert_eq!(source.len(), FILE_LEN as u64);

        let start = Barrier::new(8);
        std::thread::scope(|scope| {
            let (source, expected, start) = (&source, &expected, &start);
            for seed in 0..8_u64 {
                scope.spawn(move || {
                    let mut state = seed;
                    let mut buf = [0; 512];
                    start.wait();
                    for _ in 0..1000 {
                        state = state
                            .wrapping_mul(6_364_136_223_846_793_005)
                            .wrapping_add(1);
                        let len = 1 + ((state >> 32) as usize % buf.len());
                        let offset = (state >> 44) as usize % (expected.len() - len + 1);
                        let mut filled = 0;
                        while filled < len {
                            let n = source
                                .read_at((offset + filled) as u64, &mut buf[filled..len])
                                .unwrap();
                            assert!(n > 0, "unexpected EOF at offset {}", offset + filled);
                            filled += n;
                        }
                        assert_eq!(&buf[..len], &expected[offset..offset + len]);
                    }
                });
            }
        });
    }
}
