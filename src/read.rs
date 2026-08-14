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

/// The first read.
///
/// This is a PERFORMANCE knob and not a correctness one: every parser calls
/// `ensure` when it needs more, so a window that is too small costs a second
/// read rather than a wrong answer. Measured over 160 real files, the work per
/// file is dominated by this read rather than by parsing, so it wants to be as
/// small as usually-sufficient.
///
/// 16 KB covers the ISO BMFF `meta` box on every camera measured (a Fujifilm
/// HIF puts `iloc` at byte 859) and the EXIF APP1 of most JPEGs. A JPEG APP1
/// can reach 64 KB by spec, and those files pay one more read.
const WINDOW: usize = 16 * 1024;

pub struct Source {
    /// Absent when the bytes are already in memory, which is the RAF case:
    /// its embedded JPEG has been read once and there is nothing to seek in.
    /// This was a `File::open("/dev/null")` placeholder until it was pointed
    /// at Windows, where that path does not exist and every RAF would have
    /// failed on a filesystem call it never needed to make.
    file: Option<File>,
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
        Ok(Self {
            file: Some(file),
            window,
            len,
        })
    }

    /// Build a Source over bytes already in memory.
    ///
    /// Used for the JPEG embedded inside a RAF, which has been read once and
    /// should not be read again.
    pub fn from_vec(bytes: Vec<u8>) -> Self {
        let len = bytes.len() as u64;
        Self {
            file: None,
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

    /// Grow the front window to at least `want` bytes.
    ///
    /// A JPEG or HEIF never needs this: their EXIF is a small block near the
    /// front and 128 KB covers it. A TIFF-family file (DNG, TIFF) is different,
    /// because the file IS the directory structure and a value can be addressed
    /// anywhere in it. Growing on demand keeps the common case at one read
    /// while staying correct on the uncommon one.
    pub fn ensure(&mut self, want: usize) -> std::io::Result<()> {
        let want = want.min(self.len as usize);
        if want <= self.window.len() {
            return Ok(());
        }
        let Some(file) = self.file.as_mut() else {
            return Ok(()); // already fully in memory
        };
        let have = self.window.len();
        self.window.resize(want, 0);
        file.seek(SeekFrom::Start(have as u64))?;
        file.read_exact(&mut self.window[have..])?;
        Ok(())
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
        let Some(file) = self.file.as_mut() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "range past an in-memory source",
            ));
        };
        let mut buf = vec![0u8; len];
        file.seek(SeekFrom::Start(offset))?;
        file.read_exact(&mut buf)?;
        Ok(buf)
    }
}
