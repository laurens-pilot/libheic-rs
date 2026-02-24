use std::error::Error;
use std::fmt::{Display, Formatter};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Shared random-access source abstraction for HEIF payload ingestion.
pub trait RandomAccessSource {
    /// Total source length in bytes.
    fn len(&self) -> u64;

    /// Whether the source has zero bytes.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Read an exact byte range into `output`.
    fn read_exact_at(&mut self, offset: u64, output: &mut [u8]) -> Result<(), SourceReadError>;

    /// Read an exact byte range and return owned bytes.
    fn read_range(&mut self, offset: u64, len: usize) -> Result<Vec<u8>, SourceReadError> {
        let mut output = vec![0_u8; len];
        self.read_exact_at(offset, &mut output)?;
        Ok(output)
    }
}

impl<T: RandomAccessSource + ?Sized> RandomAccessSource for &mut T {
    fn len(&self) -> u64 {
        (**self).len()
    }

    fn read_exact_at(&mut self, offset: u64, output: &mut [u8]) -> Result<(), SourceReadError> {
        (**self).read_exact_at(offset, output)
    }
}

#[derive(Debug)]
pub enum SourceReadError {
    RangeOverflow {
        offset: u64,
        requested: usize,
    },
    OutOfBounds {
        offset: u64,
        requested: usize,
        source_len: u64,
    },
    Io {
        operation: &'static str,
        offset: u64,
        requested: usize,
        source: std::io::Error,
    },
}

impl Display for SourceReadError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            SourceReadError::RangeOverflow { offset, requested } => {
                write!(
                    f,
                    "source range overflow while reading {requested} bytes at offset {offset}"
                )
            }
            SourceReadError::OutOfBounds {
                offset,
                requested,
                source_len,
            } => write!(
                f,
                "source read out of bounds: requested {requested} bytes at offset {offset}, source length is {source_len}"
            ),
            SourceReadError::Io {
                operation,
                offset,
                requested,
                source,
            } => write!(
                f,
                "source {operation} failed for {requested} bytes at offset {offset}: {source}"
            ),
        }
    }
}

impl Error for SourceReadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            SourceReadError::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

fn checked_range_end(offset: u64, requested: usize) -> Result<u64, SourceReadError> {
    let requested_u64 = u64::try_from(requested)
        .map_err(|_| SourceReadError::RangeOverflow { offset, requested })?;
    offset
        .checked_add(requested_u64)
        .ok_or(SourceReadError::RangeOverflow { offset, requested })
}

fn validate_range(offset: u64, requested: usize, source_len: u64) -> Result<(), SourceReadError> {
    let end = checked_range_end(offset, requested)?;
    if end > source_len {
        return Err(SourceReadError::OutOfBounds {
            offset,
            requested,
            source_len,
        });
    }
    Ok(())
}

/// In-memory borrowed source implementation.
#[derive(Clone, Copy, Debug)]
pub struct SliceSource<'a> {
    data: &'a [u8],
}

impl<'a> SliceSource<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data }
    }
}

impl RandomAccessSource for SliceSource<'_> {
    fn len(&self) -> u64 {
        self.data.len() as u64
    }

    fn read_exact_at(&mut self, offset: u64, output: &mut [u8]) -> Result<(), SourceReadError> {
        validate_range(offset, output.len(), self.len())?;
        let start = usize::try_from(offset).map_err(|_| SourceReadError::OutOfBounds {
            offset,
            requested: output.len(),
            source_len: self.len(),
        })?;
        let end = start + output.len();
        output.copy_from_slice(&self.data[start..end]);
        Ok(())
    }
}

/// Generic seek-backed source implementation.
#[derive(Debug)]
pub struct SeekableSource<R: Read + Seek> {
    reader: R,
    len: u64,
}

impl<R: Read + Seek> SeekableSource<R> {
    pub fn new(mut reader: R) -> Result<Self, SourceReadError> {
        let len = reader
            .seek(SeekFrom::End(0))
            .map_err(|source| SourceReadError::Io {
                operation: "seek-end",
                offset: 0,
                requested: 0,
                source,
            })?;
        reader
            .seek(SeekFrom::Start(0))
            .map_err(|source| SourceReadError::Io {
                operation: "seek-start",
                offset: 0,
                requested: 0,
                source,
            })?;
        Ok(Self { reader, len })
    }
}

impl<R: Read + Seek> RandomAccessSource for SeekableSource<R> {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_exact_at(&mut self, offset: u64, output: &mut [u8]) -> Result<(), SourceReadError> {
        validate_range(offset, output.len(), self.len)?;
        self.reader
            .seek(SeekFrom::Start(offset))
            .map_err(|source| SourceReadError::Io {
                operation: "seek-read",
                offset,
                requested: output.len(),
                source,
            })?;
        self.reader
            .read_exact(output)
            .map_err(|source| SourceReadError::Io {
                operation: "read-exact",
                offset,
                requested: output.len(),
                source,
            })?;
        Ok(())
    }
}

pub type FileSource = SeekableSource<File>;

impl FileSource {
    pub fn open(path: &Path) -> Result<Self, SourceReadError> {
        let file = File::open(path).map_err(|source| SourceReadError::Io {
            operation: "file-open",
            offset: 0,
            requested: 0,
            source,
        })?;
        Self::new(file)
    }
}

#[cfg(test)]
mod tests {
    use super::{FileSource, RandomAccessSource, SeekableSource, SliceSource, SourceReadError};
    use std::fs;
    use std::io::{Cursor, Read, Seek, SeekFrom};
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_path() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("libheic-rs-source-{nanos}.bin"))
    }

    #[test]
    fn slice_source_reads_exact_ranges() {
        let mut source = SliceSource::new(b"0123456789");
        let mut output = [0_u8; 4];
        source
            .read_exact_at(3, &mut output)
            .expect("slice source should read exact range");
        assert_eq!(&output, b"3456");

        let read_back = source
            .read_range(0, 3)
            .expect("slice source read_range should succeed");
        assert_eq!(read_back, b"012");
    }

    #[test]
    fn slice_source_reports_out_of_bounds() {
        let mut source = SliceSource::new(b"abcd");
        let mut output = [0_u8; 5];
        let err = source
            .read_exact_at(0, &mut output)
            .expect_err("slice source should reject out-of-bounds read");
        match err {
            SourceReadError::OutOfBounds {
                offset,
                requested,
                source_len,
            } => {
                assert_eq!(offset, 0);
                assert_eq!(requested, 5);
                assert_eq!(source_len, 4);
            }
            other => panic!("expected OutOfBounds error, got {other:?}"),
        }
    }

    #[test]
    fn slice_source_reports_range_overflow() {
        let mut source = SliceSource::new(b"abcd");
        let mut output = [0_u8; 2];
        let err = source
            .read_exact_at(u64::MAX, &mut output)
            .expect_err("slice source should reject range-overflow reads");
        match err {
            SourceReadError::RangeOverflow { offset, requested } => {
                assert_eq!(offset, u64::MAX);
                assert_eq!(requested, 2);
            }
            other => panic!("expected RangeOverflow error, got {other:?}"),
        }
    }

    #[derive(Debug)]
    struct TruncatedSeekReader {
        data: Vec<u8>,
        position: u64,
        reported_len: u64,
    }

    impl TruncatedSeekReader {
        fn new(data: &[u8], reported_len: u64) -> Self {
            Self {
                data: data.to_vec(),
                position: 0,
                reported_len,
            }
        }
    }

    impl Read for TruncatedSeekReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let start = usize::try_from(self.position).map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "read position does not fit in usize",
                )
            })?;
            if start >= self.data.len() {
                return Ok(0);
            }
            let available = self.data.len() - start;
            let count = available.min(buf.len());
            buf[..count].copy_from_slice(&self.data[start..start + count]);
            self.position = self.position.checked_add(count as u64).ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "position overflow")
            })?;
            Ok(count)
        }
    }

    impl Seek for TruncatedSeekReader {
        fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
            let next = match pos {
                SeekFrom::Start(offset) => offset,
                SeekFrom::End(0) => self.reported_len,
                SeekFrom::Current(0) => self.position,
                _ => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "unsupported seek",
                    ))
                }
            };
            self.position = next;
            Ok(next)
        }
    }

    #[test]
    fn seekable_source_reads_non_contiguous_ranges() {
        let cursor = Cursor::new(b"abcdefghijklmnopqrstuvwxyz".to_vec());
        let mut source = SeekableSource::new(cursor).expect("cursor should initialize source");

        let mut first = [0_u8; 5];
        source
            .read_exact_at(2, &mut first)
            .expect("first seek-backed read should succeed");
        assert_eq!(&first, b"cdefg");

        let mut second = [0_u8; 4];
        source
            .read_exact_at(20, &mut second)
            .expect("second seek-backed read should succeed");
        assert_eq!(&second, b"uvwx");
    }

    #[test]
    fn seekable_source_reports_truncated_read_as_io_error() {
        let reader = TruncatedSeekReader::new(b"abc", 8);
        let mut source = SeekableSource::new(reader).expect("source should initialize");
        let mut output = [0_u8; 4];

        let err = source
            .read_exact_at(2, &mut output)
            .expect_err("truncated seek-backed reads should fail");
        match err {
            SourceReadError::Io {
                operation,
                offset,
                requested,
                source,
            } => {
                assert_eq!(operation, "read-exact");
                assert_eq!(offset, 2);
                assert_eq!(requested, 4);
                assert_eq!(source.kind(), std::io::ErrorKind::UnexpectedEof);
            }
            other => panic!("expected Io error, got {other:?}"),
        }
    }

    #[test]
    fn file_source_reads_exact_ranges_from_disk() {
        let path = unique_temp_path();
        fs::write(&path, b"heif-source-layer").expect("temp source fixture should be writable");
        let mut source = FileSource::open(&path).expect("file source should open");

        let mut output = [0_u8; 6];
        source
            .read_exact_at(5, &mut output)
            .expect("file source read should succeed");
        assert_eq!(&output, b"source");

        fs::remove_file(&path).expect("temp source fixture should be removed");
    }
}
