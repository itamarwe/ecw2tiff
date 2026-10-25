//! Positional file access and buffered byte cursors over file ranges.

use super::{Error, Result};
use std::fs::File;
use std::path::Path;

/// A read-only file accessed exclusively through positional reads, so that
/// any number of threads can read from it at once.
pub(crate) struct FileHandle {
    #[cfg(any(unix, windows))]
    file: File,
    #[cfg(not(any(unix, windows)))]
    file: std::sync::Mutex<File>,
    len: u64,
}

impl FileHandle {
    pub(crate) fn open(path: &Path) -> Result<FileHandle> {
        let file = File::open(path).map_err(|e| Error::io(&e))?;
        let len = file.metadata().map_err(|e| Error::io(&e))?.len();
        #[cfg(not(any(unix, windows)))]
        let file = std::sync::Mutex::new(file);
        Ok(FileHandle { file, len })
    }

    pub(crate) fn len(&self) -> u64 {
        self.len
    }

    /// Fill `buf` from file offset `pos`. Fails if the range is not entirely
    /// inside the file.
    pub(crate) fn read_at(&self, pos: u64, buf: &mut [u8]) -> Result<()> {
        let end = pos.checked_add(buf.len() as u64);
        if end.is_none_or(|e| e > self.len) {
            return Err(Error::malformed("read beyond end of file"));
        }
        self.read_raw(pos, buf).map_err(|e| Error::io(&e))
    }

    #[cfg(unix)]
    fn read_raw(&self, pos: u64, buf: &mut [u8]) -> std::io::Result<()> {
        use std::os::unix::fs::FileExt;
        self.file.read_exact_at(buf, pos)
    }

    #[cfg(windows)]
    fn read_raw(&self, mut pos: u64, mut buf: &mut [u8]) -> std::io::Result<()> {
        use std::os::windows::fs::FileExt;
        while !buf.is_empty() {
            match self.file.seek_read(buf, pos) {
                Ok(0) => return Err(std::io::ErrorKind::UnexpectedEof.into()),
                Ok(n) => {
                    let rest = buf;
                    buf = &mut rest[n..];
                    pos += n as u64;
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    #[cfg(not(any(unix, windows)))]
    fn read_raw(&self, pos: u64, buf: &mut [u8]) -> std::io::Result<()> {
        use std::io::{Read, Seek, SeekFrom};
        let mut f = self
            .file
            .lock()
            .map_err(|_| std::io::Error::other("file lock poisoned"))?;
        f.seek(SeekFrom::Start(pos))?;
        f.read_exact(buf)
    }

    /// Read `len` bytes at `pos` into a new vector.
    pub(crate) fn read_vec(&self, pos: u64, len: u64) -> Result<Vec<u8>> {
        let end = pos.checked_add(len);
        if end.is_none_or(|e| e > self.len) {
            return Err(Error::malformed("read beyond end of file"));
        }
        let n = usize::try_from(len).map_err(|_| Error::malformed("block too large"))?;
        let mut v = Vec::new();
        v.try_reserve_exact(n)
            .map_err(|_| Error::new(super::ErrorKind::Io, "out of memory"))?;
        v.resize(n, 0);
        self.read_raw(pos, &mut v).map_err(|e| Error::io(&e))?;
        Ok(v)
    }
}

/// Largest number of bytes fetched from the file at once by a [`ByteSource`].
const CHUNK: u64 = 16 * 1024;

/// Sequential reader over the file range `[start, end)`, fetching it in
/// chunks on demand. Reads beyond `end` are reported as `None` by
/// [`ByteSource::next`] and as zero bytes by [`ByteSource::next_or_zero`].
pub(crate) struct ByteSource<'f> {
    file: &'f FileHandle,
    /// File offset just after the bytes currently held in `buf`.
    fetched_to: u64,
    end: u64,
    buf: Vec<u8>,
    pos: usize,
    /// Set when a read from the file failed; the stream is then unusable.
    failed: bool,
}

impl<'f> ByteSource<'f> {
    /// A cursor at `start`, with `prefix` already holding the bytes of the
    /// file from `start` on (it may be empty; it must not reach past `end`).
    pub(crate) fn new(file: &'f FileHandle, start: u64, end: u64, prefix: Vec<u8>) -> Self {
        let fetched_to = start.saturating_add(prefix.len() as u64);
        ByteSource {
            file,
            fetched_to,
            end,
            buf: prefix,
            pos: 0,
            failed: false,
        }
    }

    /// True when a file read failed.
    pub(crate) fn failed(&self) -> bool {
        self.failed
    }

    /// Fetch the next chunk; returns false at the end of the range or on failure.
    #[cold]
    fn refill(&mut self) -> bool {
        if self.failed || self.fetched_to >= self.end {
            return false;
        }
        let n = (self.end - self.fetched_to).min(CHUNK) as usize;
        self.buf.clear();
        self.buf.resize(n, 0);
        self.pos = 0;
        if self.file.read_at(self.fetched_to, &mut self.buf).is_err() {
            self.failed = true;
            self.buf.clear();
            return false;
        }
        self.fetched_to += n as u64;
        true
    }

    /// Next byte, or `None` past the end of the range (or after a failure).
    #[inline]
    pub(crate) fn next(&mut self) -> Option<u8> {
        if self.pos >= self.buf.len() && !self.refill() {
            return None;
        }
        let b = self.buf.get(self.pos).copied();
        self.pos += 1;
        b
    }

    /// Next byte, or 0 past the end of the range.
    #[inline(always)]
    pub(crate) fn next_or_zero(&mut self) -> u8 {
        if let Some(&b) = self.buf.get(self.pos) {
            self.pos += 1;
            b
        } else {
            self.next().unwrap_or(0)
        }
    }

    /// Next two bytes as a little-endian u16, or `None` if fewer remain.
    #[inline]
    pub(crate) fn next_u16le(&mut self) -> Option<u16> {
        if let Some(s) = self.buf.get(self.pos..self.pos + 2) {
            let v = u16::from_le_bytes([s[0], s[1]]);
            self.pos += 2;
            return Some(v);
        }
        let lo = self.next()?;
        let hi = self.next()?;
        Some(u16::from_le_bytes([lo, hi]))
    }
}
