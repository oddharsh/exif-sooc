//! Bounded file reading.
//!
//! This is the whole performance argument, so it is the first file to read.
//!
//! Every field this tool extracts lives near the front of the file. Measured on
//! a 16 MB Fujifilm HIF, the `iloc` box that points at the EXIF sits at byte
//! 859, and the EXIF payload itself is under 8 KB. Reading the file to a `Vec`
//! costs 16 MB of I/O to use 0.05% of it, and over a folder of 114 that is
//! 1.9 GB read to use about 1 MB.
//!
//! So nothing here ever reads a whole file. A `Source` keeps one window at the
//! front, which answers almost every question, and seeks for the rare read that
//! falls outside it.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// How much of the front of a file to pull in one go.
///
/// A JPEG's EXIF APP1 segment cannot exceed 64 KB by the JPEG spec, and the
/// ISO BMFF `meta` box is written before `mdat` by every camera measured, so
/// 128 KB answers both without a second read.
const WINDOW: usize = 128 * 1024;

pub struct Source {
    file: File,
    /// The first `window.len()` bytes of the file.
    window: Vec<u8>,
    len: u64,
}

impl Source {
    pub fn open(path: &Path) -> std::io::Result<Self> {
        let mut file = File::open(path)?;
        let len = file.metadata()?.len();
        let want = WINDOW.min(len as usize);
        let mut window = vec![0u8; want];
        file.read_exact(&mut window)?;
        Ok(Self { file, window, len })
    }

    /// Build a Source over bytes already in memory.
    ///
    /// Used for the JPEG embedded inside a RAF, which has been read once and
    /// should not be read again.
    pub fn from_vec(bytes: Vec<u8>) -> Self {
        let len = bytes.len() as u64;
        Self {
            // A File is required by the struct and is never touched when the
            // window covers the whole source, which it does here.
            file: File::open("/dev/null").expect("/dev/null"),
            window: bytes,
            len,
        }
    }

    pub fn len(&self) -> u64 {
        self.len
    }

    /// The front window, for parsers that walk structures from the start.
    pub fn front(&self) -> &[u8] {
        &self.window
    }

    /// Read exactly `len` bytes at `offset`, seeking only when the range falls
    /// outside the window.
    pub fn range(&mut self, offset: u64, len: usize) -> std::io::Result<Vec<u8>> {
        let end = offset.saturating_add(len as u64);
        if end > self.len {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "range past end of file",
            ));
        }
        if end <= self.window.len() as u64 {
            let start = offset as usize;
            return Ok(self.window[start..start + len].to_vec());
        }
        let mut buf = vec![0u8; len];
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.read_exact(&mut buf)?;
        Ok(buf)
    }
}
