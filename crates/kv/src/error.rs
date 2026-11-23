//! The errors that every engine returns.
//!
//! Code above storage often holds an engine as `dyn Kv`, so it cannot know
//! the engine's own error type. What it needs to know is what to do next,
//! and each variant answers that:
//!
//! | Variant | What the caller does |
//! |---|---|
//! | `KeyTooLong`, `ValueTooLong` | Fix the input. Nothing was written. |
//! | `Vfs` | A file call failed. Open the engine again, or stop. |
//! | `Stopped` | An earlier failure stopped the engine. Open it again. |
//! | `Corrupt` | The files are damaged. Stop and check them. |
//!
//! An engine turns its own errors into these with `From`.

use std::{error, fmt, result};

use crate::{MAX_KEY, MAX_VALUE};

/// Everything that can go wrong in a [`Kv`](crate::Kv) call or with a
/// [`Batch`](crate::Batch).
///
/// # Examples
///
/// ```
/// use kv::{Error, MAX_KEY};
///
/// let err = Error::KeyTooLong { len: 513, max: MAX_KEY };
/// assert_eq!(err.to_string(), "the key is 513 bytes long, and the limit is 512");
/// ```
#[derive(Debug)]
pub enum Error {
    /// A key is longer than [`MAX_KEY`]. The batch did not take it.
    KeyTooLong {
        /// The key's length, in bytes.
        len: usize,
        /// The limit, [`MAX_KEY`].
        max: usize,
    },
    /// A value is longer than [`MAX_VALUE`]. The batch did not take it.
    ValueTooLong {
        /// The value's length, in bytes.
        len: usize,
        /// The limit, [`MAX_VALUE`].
        max: usize,
    },
    /// A file system call failed. The inner error names the call and the
    /// path. The engine may have stopped, and then it returns
    /// [`Error::Stopped`] until it is opened again.
    Vfs(vfs::Error),
    /// The engine's files break a rule that the engine always keeps when it
    /// writes them, so something else changed them. The text says which
    /// rule broke, with the numbers.
    Corrupt(String),
    /// An earlier write or sync failed, so the engine no longer knows what
    /// is on disk, and it refuses every write. Open the engine again: it
    /// reads what is on disk.
    Stopped,
}

/// The result of every fallible function in this crate, and of every
/// [`Kv`](crate::Kv) method.
pub type Result<T> = result::Result<T, Error>;

impl Error {
    /// Checks a key's length against [`MAX_KEY`].
    pub(crate) fn check_key(key: &[u8]) -> Result<()> {
        match key.len() {
            len if len > MAX_KEY => Err(Error::KeyTooLong { len, max: MAX_KEY }),
            _ => Ok(()),
        }
    }

    /// Checks a value's length against [`MAX_VALUE`].
    pub(crate) fn check_value(value: &[u8]) -> Result<()> {
        match value.len() {
            len if len > MAX_VALUE => Err(Error::ValueTooLong {
                len,
                max: MAX_VALUE,
            }),
            _ => Ok(()),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::KeyTooLong { len, max } => {
                write!(f, "the key is {len} bytes long, and the limit is {max}")
            }
            Error::ValueTooLong { len, max } => {
                write!(f, "the value is {len} bytes long, and the limit is {max}")
            }
            Error::Vfs(e) => write!(f, "{e}"),
            Error::Corrupt(reason) => write!(f, "corrupt: {reason}"),
            Error::Stopped => write!(
                f,
                "the engine stopped after an earlier write or sync failed; open it again"
            ),
        }
    }
}

impl error::Error for Error {}

// With this impl, `?` on a file system call turns a `vfs::Error` into an
// `Error`.
impl From<vfs::Error> for Error {
    fn from(e: vfs::Error) -> Error {
        Error::Vfs(e)
    }
}

// The model engine keeps its batches in a `wal` log. Each of the log's
// errors has a variant here with the same meaning.
impl From<wal::Error> for Error {
    fn from(e: wal::Error) -> Error {
        match e {
            wal::Error::Vfs(e) => Error::Vfs(e),
            wal::Error::Stopped => Error::Stopped,
            wal::Error::Corrupt { path, reason } => {
                Error::Corrupt(format!("{}: {reason}", path.display()))
            }
        }
    }
}
