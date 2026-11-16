//! The errors every file operation can return.
//!
//! Each variant says what was being done and to which path, and `fmt` turns
//! it into a message that says both.
//!
//! The common errors from the operating system, such as a missing file, get
//! their own variants, so the same failure looks the same from `Disk` and
//! from `Mem`. Everything else is [`Error::Io`].
//!
//! That matters because tests run on `Mem` and production runs on `Disk`.
//! If a missing file were a `NotFound` variant on one and an `io::Error`
//! on the other, a caller's `match` would work in tests and fail in
//! production. With shared variants, code that handles an error on one
//! handles it on both.

use std::{error, fmt, io, path::PathBuf, result};

use crate::Fault;

/// Everything that can go wrong with a file or a directory.
///
/// Callers that need to react to one case match on its variant, such as
/// `NotFound` when a file may not exist yet. The others are best passed up
/// with `?`, because their message already says what happened and where.
///
/// # Examples
///
/// ```
/// use std::path::Path;
/// use vfs::{Disk, Error, Open, Vfs};
///
/// # let root = std::env::temp_dir().join(format!("vfs-doc-{}", std::process::id()));
/// # std::fs::create_dir_all(&root).unwrap();
/// let fs = Disk::new(&root);
/// // A file that may not exist yet: create it if opening finds nothing.
/// let file = match fs.open(Path::new("a.db"), Open::Existing) {
///     Err(Error::NotFound(_)) => fs.open(Path::new("a.db"), Open::CreateNew).unwrap(),
///     Ok(file) => file,
///     Err(e) => panic!("{e}"),
/// };
/// assert_eq!(file.len().unwrap(), 0);
///
/// // Every error's message names the path.
/// let Err(e) = fs.open(Path::new("b.db"), Open::Existing) else { panic!() };
/// assert_eq!(e.to_string(), "b.db: no such file or directory");
/// # std::fs::remove_dir_all(&root).unwrap();
/// ```
#[derive(Debug)]
pub enum Error {
    /// Any other error from the operating system, such as a full disk or a
    /// permission problem.
    Io {
        /// What was being done: "open", "write", "sync" and so on.
        op: &'static str,
        /// The path it was done to.
        path: PathBuf,
        /// What the operating system said.
        source: io::Error,
    },
    /// Nothing exists at this path.
    NotFound(PathBuf),
    /// Something already exists at this path.
    AlreadyExists(PathBuf),
    /// The path is a directory where a file was needed, or a file where a
    /// directory was needed.
    WrongType(PathBuf),
    /// The path is absolute, empty where a name is needed, or has `.` or `..`
    /// in it.
    BadPath(PathBuf),
    /// A rename between two different directories.
    CrossDirectory {
        /// The file being renamed.
        from: PathBuf,
        /// The name it was to get.
        to: PathBuf,
    },
    /// Another handle holds the lock on this file.
    Locked(PathBuf),
    /// A read asked for bytes past the end of the file. Nothing useful was
    /// read.
    ShortRead {
        /// The file.
        path: PathBuf,
        /// Where the read started, in bytes.
        offset: u64,
        /// How many bytes it asked for.
        len: usize,
        /// The file's length, in bytes.
        file_len: u64,
    },
    /// The file handle was opened before a crash, so it no longer works.
    Stale(PathBuf),
    /// The power is off. [`Mem::power_off_after`](crate::Mem::power_off_after)
    /// set how many changes could happen, and they are used up. Every change
    /// fails until [`Mem::crash`](crate::Mem::crash).
    PowerOff,
    /// A fault injected with [`Mem::fail_next`](crate::Mem::fail_next).
    Injected(Fault),
}

/// The result of every fallible function in this crate.
pub type Result<T> = result::Result<T, Error>;

impl Error {
    /// Turns an `io::Error` from `op` on `path` into an `Error`. Common
    /// errors, like a missing file, get their own variant, and the rest
    /// become [`Error::Io`]. A file system built on `std::fs` uses it for
    /// every error it gets.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::io;
    /// use vfs::Error;
    ///
    /// let e = io::Error::from(io::ErrorKind::NotFound);
    /// assert!(matches!(Error::from_io("open", "a.db", e), Error::NotFound(_)));
    ///
    /// let e = io::Error::from(io::ErrorKind::StorageFull);
    /// let e = Error::from_io("write", "a.db", e);
    /// assert!(e.to_string().starts_with("write a.db: "));
    /// ```
    pub fn from_io(op: &'static str, path: impl Into<PathBuf>, source: io::Error) -> Error {
        let path = path.into();
        match source.kind() {
            io::ErrorKind::NotFound => Error::NotFound(path),
            io::ErrorKind::AlreadyExists => Error::AlreadyExists(path),
            io::ErrorKind::IsADirectory | io::ErrorKind::NotADirectory => Error::WrongType(path),
            _ => Error::Io { op, path, source },
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io { op, path, source } => write!(f, "{op} {}: {source}", path.display()),
            Error::NotFound(path) => write!(f, "{}: no such file or directory", path.display()),
            Error::AlreadyExists(path) => write!(f, "{}: already exists", path.display()),
            Error::WrongType(path) => write!(
                f,
                "{}: a file where a directory was needed, or the other way round",
                path.display()
            ),
            Error::BadPath(path) => {
                write!(f, "{:?}: paths must be relative, with no `.` or `..`", path)
            }
            Error::CrossDirectory { from, to } => {
                write!(
                    f,
                    "rename {} to {}: both must be in the same directory",
                    from.display(),
                    to.display()
                )
            }
            Error::Locked(path) => write!(f, "{}: another handle holds the lock", path.display()),
            Error::ShortRead {
                path,
                offset,
                len,
                file_len,
            } => write!(
                f,
                "read {}: {len} bytes at {offset} runs past the end of the file, which is {file_len} bytes",
                path.display()
            ),
            Error::Stale(path) => write!(
                f,
                "{}: this handle was opened before a crash",
                path.display()
            ),
            Error::PowerOff => write!(f, "the power is off"),
            Error::Injected(fault) => write!(f, "injected fault: {}", fault.name()),
        }
    }
}

impl error::Error for Error {}
