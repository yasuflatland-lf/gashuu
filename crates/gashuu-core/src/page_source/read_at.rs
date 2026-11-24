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

    mod props {
        use super::*;
        use proptest::prelude::*;
        use std::io::Cursor;

        #[derive(Clone, Debug)]
        enum CursorOp {
            Read(usize),
            Seek(SeekFrom),
            CloneAndSwitch,
        }

        fn cursor_input() -> impl Strategy<Value = (Vec<u8>, Vec<CursorOp>)> {
            proptest::collection::vec(any::<u8>(), 0..64).prop_flat_map(|data| {
                let bound = data.len() as i64 + 2;
                let delta = prop_oneof![-bound..=2, Just(i64::MIN), Just(i64::MAX)];
                let start = prop_oneof![0..=bound as u64, Just(u64::MAX)];
                let op = prop_oneof![
                    (0usize..80).prop_map(CursorOp::Read),
                    start.prop_map(|offset| CursorOp::Seek(SeekFrom::Start(offset))),
                    delta
                        .clone()
                        .prop_map(|offset| CursorOp::Seek(SeekFrom::End(offset))),
                    delta.prop_map(|offset| CursorOp::Seek(SeekFrom::Current(offset))),
                    Just(CursorOp::CloneAndSwitch),
                ];
                (Just(data), proptest::collection::vec(op, 0..=60))
            })
        }

        proptest! {
            /// C2-9: An arc-backed cursor reads, seeks and clones like an in-memory cursor.
            /// Replaces `cursor_reads_sequentially_and_stops_at_eof`,
            /// `cursor_seek_start_end_current`, `cursor_seek_before_start_is_invalid_input`,
            /// `cursor_clone_has_independent_position`, and
            /// `arc_slice_read_at_offsets_short_tail_and_past_end`.
            #[test]
            fn prop_read_at_cursor_matches_std_cursor((data, operations) in cursor_input()) {
                let bytes: Arc<[u8]> = Arc::from(data.as_slice());
                prop_assert_eq!(ReadAt::len(&bytes), data.len() as u64);
                prop_assert_eq!(ReadAt::is_empty(&bytes), data.is_empty());
                let mut actual = ReadAtCursor::new(Arc::new(bytes));
                let mut expected = Cursor::new(data.as_slice());
                let mut parked = None;

                for operation in operations {
                    match operation {
                        CursorOp::Read(len) => {
                            let mut actual_buf = [0; 80];
                            let mut expected_buf = [0; 80];
                            let actual_len = actual.read(&mut actual_buf[..len]).unwrap();
                            let expected_len = expected.read(&mut expected_buf[..len]).unwrap();
                            prop_assert_eq!(actual_len, expected_len);
                            prop_assert_eq!(&actual_buf[..actual_len], &expected_buf[..expected_len]);
                        }
                        CursorOp::Seek(seek) => {
                            let before = actual.stream_position().unwrap();
                            let result = actual.seek(seek).map_err(|error| error.kind());
                            let expected_result = expected.seek(seek).map_err(|error| error.kind());
                            prop_assert_eq!(result, expected_result);
                            if result.is_err() {
                                prop_assert_eq!(result, Err(io::ErrorKind::InvalidInput));
                                prop_assert_eq!(actual.stream_position().unwrap(), before);
                            }
                        }
                        CursorOp::CloneAndSwitch => {
                            let clone = actual.clone();
                            parked = Some((std::mem::replace(&mut actual, clone), expected.position()));
                            expected = expected.clone();
                        }
                    }
                    prop_assert_eq!(actual.stream_position().unwrap(), expected.position());
                    if let Some((original, position)) = &mut parked {
                        prop_assert_eq!(original.stream_position().unwrap(), *position);
                    }
                }
            }
        }
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
