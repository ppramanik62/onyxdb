//! The errors the pool can return.
//!
//! Every fallible function in this crate returns [`Result`]. Errors from the
//! page file below come back wrapped in [`Error::PageFile`], so a caller
//! matches on one enum.

use std::{error, fmt, result};

/// Everything that can go wrong fetching, writing or flushing a page.
///
/// After any of these errors, the pool still works. The failed call may
/// already have written back or evicted a page to make room, but every page
/// in the pool is still correct. The caller can retry the call or continue
/// with other work.
///
/// Every page file error goes inside one variant, `PageFile`. The pool does
/// not copy each page file error into a variant of its own. The page file
/// already has one variant per rule, with the numbers, and a copy here
/// would have to change every time that list changed. The pool adds only
/// the errors that it causes itself.
///
/// # Examples
///
/// ```
/// # use bufpool::{BufPool, Clock, NoLog};
/// # use pagefile::{PageFile, PageNo};
/// # use vfs::Mem;
/// # let mem = Mem::new();
/// # let file = PageFile::create(&mem, "db", 512).unwrap();
/// use bufpool::Error;
///
/// // A file with only the header page, and a pool in front of it.
/// let pool = BufPool::new(file, 2, Clock::new, Box::new(NoLog));
/// // Page 1 isn't in the file, so loading it fails in the page file.
/// let Err(e) = pool.fetch(PageNo::new(1).unwrap()) else { panic!() };
/// assert!(matches!(e, Error::PageFile(_)));
/// ```
#[derive(Debug)]
pub enum Error {
    /// The page file failed. A read or a write went wrong, a page's checksum
    /// did not match when it was loaded, or the page was past the end of the
    /// file and the read came up short. Through `PageStore`, it can also
    /// mean a page number at or past the header's page count. The inner
    /// error says which one happened.
    PageFile(pagefile::Error),
    /// A page had to be loaded, but every frame holds a pinned page, so
    /// there is no frame to put it in. Dropping a guard unpins a frame and
    /// makes room. A pool needs at least as many frames as the largest
    /// number of guards that callers hold at the same time.
    AllPinned {
        /// The number of frames in the pool.
        frames: usize,
    },
    /// The log hook failed to make the log durable, so a dirty page
    /// could not be written. The page stays dirty in its frame. The inner
    /// error is the one the hook returned.
    Log(Box<dyn error::Error + Send + Sync>),
}

/// The result of every fallible function in this crate.
pub type Result<T> = result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::PageFile(e) => write!(f, "{e}"),
            Error::AllPinned { frames } => write!(
                f,
                "no frame is free: all {frames} frames hold a pinned page"
            ),
            Error::Log(e) => write!(f, "the log couldn't be flushed: {e}"),
        }
    }
}

impl error::Error for Error {}

// With this impl, `?` on a page file operation turns a `pagefile::Error`
// into an `Error`.
impl From<pagefile::Error> for Error {
    fn from(value: pagefile::Error) -> Self {
        Error::PageFile(value)
    }
}
