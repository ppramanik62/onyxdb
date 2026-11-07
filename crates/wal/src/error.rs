//! The errors the log can return.
//!
//! Every fallible function in this crate returns [`Result`]. Errors from the
//! file system come back wrapped in [`Error::Vfs`], so a caller matches on
//! one enum.

use std::{error, fmt};

/// Everything that can go wrong creating, opening, appending to, syncing or
/// reading the log.
///
/// A failed write, sync, create or remove stops the log. That call returns
/// the file system's error, and every later call returns
/// [`Error::Stopped`] until the log is opened again.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use vfs::{Fault, Mem};
/// use wal::{BLOCK, Error, Wal};
///
/// let mem = Mem::new();
/// let wal = Wal::create(Arc::new(mem.clone()), "wal", BLOCK as u64).unwrap();
/// let lsn = wal.append(b"hello").unwrap();
///
/// // The disk fails the sync, so the log stops.
/// mem.fail_next(Fault::SyncError);
/// assert!(matches!(wal.flush_to(lsn), Err(Error::Vfs(_))));
/// assert!(matches!(wal.append(b"again"), Err(Error::Stopped)));
/// ```
#[derive(Debug)]
pub enum Error {
    /// A file system call failed. The inner error names the call and the path.
    Vfs(vfs::Error),
    /// The log stopped after an earlier write, sync, create or remove
    /// failed. It no longer knows which of its records are on disk, so it
    /// refuses every call. Open it again with [`Wal::open`](crate::Wal::open),
    /// which reads what's on disk.
    Stopped,
}

/// The result of every fallible function in this crate.
pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Vfs(e) => write!(f, "{e}"),
            Error::Stopped => write!(
                f,
                "the log stopped after an earlier write or sync failed; open it again"
            ),
        }
    }
}

impl error::Error for Error {}

// This lets `?` on a file system call turn its error into ours.
impl From<vfs::Error> for Error {
    fn from(e: vfs::Error) -> Error {
        Error::Vfs(e)
    }
}
