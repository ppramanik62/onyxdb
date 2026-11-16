//! Segment files: where the log's bytes are stored.
//!
//! The log must be able to delete its oldest records once they are no
//! longer needed. A file cannot remove bytes from its start. So the log is
//! stored in many files, called segments, and it deletes old segments as a
//! whole. Each segment holds a run of whole blocks. Its name is the LSN of
//! its first byte, in 16 hex digits, so the names sort in log order:
//!
//! ```text
//! wal/
//!   0000000000000000.wal   LSN 0 to 32767        full
//!   0000000000008000.wal   LSN 32768 to 65535    full
//!   0000000000010000.wal   LSN 65536 ...         the current segment
//! ```
//!
//! The last segment is the current one, where appends go. Every other
//! segment is full: it holds every byte from its own start to the start of
//! the next segment. The names say where each segment starts, so [`list`]
//! can check this rule. A missing segment, or one that is too short, breaks
//! it.

use std::path::{Path, PathBuf};

use vfs::{Open, Vfs};

use crate::{BLOCK, Error, Lsn, Result};

/// The path of the segment that starts at LSN `start`, inside `dir`.
///
/// # Examples
///
/// ```
/// use std::path::Path;
/// use wal::segment::path;
///
/// let p = path(Path::new("wal"), 65536);
/// assert_eq!(p, Path::new("wal/0000000000010000.wal"));
/// ```
pub fn path(dir: &Path, start: Lsn) -> PathBuf {
    dir.join(format!("{start:016x}.wal"))
}

/// The LSN a segment starts at, read from its file name, or `None` if
/// `name` is not a name that [`path`] makes.
///
/// # Examples
///
/// ```
/// use wal::segment::start;
///
/// assert_eq!(start("0000000000010000.wal"), Some(65536));
/// assert_eq!(start("notes.txt"), None);
/// ```
pub fn start(name: &str) -> Option<Lsn> {
    let hex = name.strip_suffix(".wal")?;
    // Accept only the 16 lowercase hex digits that `path` writes, so each
    // start LSN has exactly one name.
    if hex.len() != 16 || !hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    u64::from_str_radix(hex, 16).ok()
}

/// The start of every segment in `dir`, oldest first. Before it returns, it
/// checks that the segments together hold one log with no gaps.
///
/// It opens every segment except the last one to read its length. Returns
/// an empty list if `dir` holds no files.
///
/// # Errors
///
/// - [`Error::Vfs`] if `dir` cannot be listed, or a segment cannot be opened.
///   If `dir` does not exist, that is `vfs::Error::NotFound`.
/// - [`Error::Corrupt`] if a file in `dir` is not named like a segment, a
///   segment does not start at a block boundary, or a segment other than the
///   last does not end exactly where the next one starts.
///
/// # Examples
///
/// ```
/// use std::{path::Path, sync::Arc};
/// use vfs::Mem;
/// use wal::{BLOCK, Wal, segment};
///
/// let mem = Mem::new();
/// let wal = Wal::create(Arc::new(mem.clone()), "wal", BLOCK as u64).unwrap();
/// // A record bigger than one block fills the first segment.
/// wal.append(&[7; BLOCK]).unwrap();
/// assert_eq!(segment::list(&mem, Path::new("wal")).unwrap(), [0, BLOCK as u64]);
/// ```
pub fn list(vfs: &dyn Vfs, dir: &Path) -> Result<Vec<Lsn>> {
    let mut starts = Vec::new();
    for path in vfs.list(dir)? {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let Some(start) = start(name) else {
            let reason = "not a segment: a segment's name is 16 hex digits and .wal";
            return Err(Error::Corrupt {
                path,
                reason: reason.into(),
            });
        };
        if !start.is_multiple_of(BLOCK as u64) {
            return Err(Error::Corrupt {
                path,
                reason: format!(
                    "the segment starts at LSN {start}, which isn't a multiple of the block size {BLOCK}"
                ),
            });
        }
        starts.push(start);
    }
    // A real directory lists its files in no particular order.
    starts.sort_unstable();
    for pair in starts.windows(2) {
        let path = path(dir, pair[0]);
        let len = vfs.open(&path, Open::Existing)?.len()?;
        if pair[0] + len != pair[1] {
            return Err(Error::Corrupt {
                path,
                reason: format!(
                    "the segment holds {len} bytes, but the next one starts {} bytes after it",
                    pair[1] - pair[0]
                ),
            });
        }
    }
    Ok(starts)
}
