//! Segment files: where the log's bytes are stored.
//!
//! A file can't drop its beginning, but an old log has to be thrown away a
//! piece at a time. So the log is stored in segment files, each holding a
//! run of whole blocks. A segment is named by the LSN of its first byte, in
//! 16 hex digits, so the names sort in log order:
//!
//! ```text
//! wal/
//!   0000000000000000.wal   LSN 0 to 32767        full
//!   0000000000008000.wal   LSN 32768 to 65535    full
//!   0000000000010000.wal   LSN 65536 ...         the current segment
//! ```
//!
//! The last segment is the current one, where appends go. Every other
//! segment holds exactly the bytes up to where the next one starts. Because
//! the names say where each segment starts, [`list`] can check that, and so
//! catch a segment that's missing or cut short.

use std::path::{Path, PathBuf};

use crate::Lsn;

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
/// `name` isn't a name that [`path`] makes.
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
    // Exactly the 16 lowercase digits `path` writes, so each start has only one name.
    if hex.len() != 16 || !hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    u64::from_str_radix(hex, 16).ok()
}
