//! `Vfs` and `File`: everything a database does with files and directories.
//!
//! A `Vfs` is a file system: it opens files and changes directories. A `File`
//! is one open file: it reads and writes bytes at an offset, changes its
//! length, and syncs. Paths are relative to the root of the file system, so
//! the same path means the same file to every implementation.
//!
//! A path is made of plain names joined by `/`, like `data/pages.pf`. It
//! cannot be absolute or use `.` or `..`. The root directory is the empty
//! path.
//!
//! Paths are relative so that a `Disk` can be rooted in any directory, and
//! `.` and `..` are refused so that no path can reach outside that root. It
//! also keeps the rules simple for `Mem`: a path is a list of names, with no
//! links to follow.
//!
//! Durability takes two syncs. `File::sync` makes a file's bytes durable,
//! and `Vfs::sync_dir` makes the names in a directory durable. A new file
//! needs both, or a crash can keep its bytes and lose its name.
//!
//! ```text
//! open(CreateNew) -> write_at -> sync -> sync_dir(parent)
//!                                 |          |
//!                     bytes durable      name durable
//! ```

use std::path::{Path, PathBuf};

use crate::Result;

/// How [`Vfs::open`] treats a file that is or is not there. There is no mode
/// that does both, so a caller always says whether it expects the file.
///
/// A mode that opens a file, or creates it if it is missing, would hide a
/// mistake. A database that expects its file and does not find it should
/// report an error. It should not create a new empty file, because then the
/// user's data would be lost and nothing would say so.
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
/// let path = Path::new("a.db");
/// assert!(matches!(fs.open(path, Open::Existing), Err(Error::NotFound(_))));
/// fs.open(path, Open::CreateNew).unwrap();
/// assert!(matches!(fs.open(path, Open::CreateNew), Err(Error::AlreadyExists(_))));
/// # std::fs::remove_dir_all(&root).unwrap();
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Open {
    /// Open a file that exists. Fails with `Error::NotFound` if it does not.
    Existing,
    /// Create a new, empty file. Fails with `Error::AlreadyExists` if one is
    /// there.
    CreateNew,
}

/// A file system: somewhere files can be opened and directories changed.
///
/// Every method takes a path relative to the file system's root. Every
/// method fails with [`Error::BadPath`](crate::Error::BadPath) if the path
/// is absolute or has `.` or `..` in it.
///
/// It is `Send` and `Sync`, so threads can share one file system.
///
/// The trait is small on purpose. It has only what a database needs: open,
/// remove, rename, list, create a directory, and sync a directory. Every
/// method is one more thing `Mem` has to model through a crash, so a method
/// goes in only when something needs it.
///
/// # Examples
///
/// ```
/// use std::path::Path;
/// use vfs::{Disk, Open, Vfs};
///
/// # let root = std::env::temp_dir().join(format!("vfs-doc-{}", std::process::id()));
/// # std::fs::create_dir_all(&root).unwrap();
/// let fs = Disk::new(&root);
/// let file = fs.open(Path::new("new.db"), Open::CreateNew).unwrap();
/// file.write_at(b"hello", 0).unwrap();
/// file.sync().unwrap();
/// // The root directory is the empty path.
/// fs.sync_dir(Path::new("")).unwrap();
///
/// fs.rename(Path::new("new.db"), Path::new("live.db")).unwrap();
/// assert!(!fs.exists(Path::new("new.db")).unwrap());
/// assert_eq!(fs.list(Path::new("")).unwrap(), [Path::new("live.db")]);
/// # std::fs::remove_dir_all(&root).unwrap();
/// ```
pub trait Vfs: Send + Sync {
    /// Opens the file at `path` for reading and writing, or creates it
    /// empty, as `mode` says. A new file's name is not durable until its
    /// directory is synced.
    ///
    /// # Errors
    ///
    /// - [`Error::NotFound`](crate::Error::NotFound) with
    ///   [`Open::Existing`] if there is no file, or if the parent directory
    ///   does not exist.
    /// - [`Error::AlreadyExists`](crate::Error::AlreadyExists) with
    ///   [`Open::CreateNew`] if anything is already at `path`.
    /// - [`Error::WrongType`](crate::Error::WrongType) if `path` is a
    ///   directory.
    fn open(&self, path: &Path, mode: Open) -> Result<Box<dyn File>>;

    /// Whether a file or directory exists at `path`. The empty path, the
    /// root, always exists.
    ///
    /// # Errors
    ///
    /// Any error from the file system other than a missing file, such as a
    /// permission problem on a parent directory.
    fn exists(&self, path: &Path) -> Result<bool>;

    /// Removes the file at `path`. Handles already open keep working. The
    /// removal is not durable until the directory is synced.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`](crate::Error::NotFound) if nothing is there, and
    /// [`Error::WrongType`](crate::Error::WrongType) if it is a directory.
    fn remove(&self, path: &Path) -> Result<()>;

    /// Gives the file at `from` the name `to`, replacing any file already
    /// called `to`. Both must be in the same directory. It is atomic: after a
    /// crash, `to` names either the old file or the new one. The rename
    /// is not durable until the directory is synced.
    ///
    /// Renaming over a file is how a caller replaces a file safely: write
    /// the new contents to a temporary name, sync it, rename it over the old
    /// name, and sync the directory. That is why the rename must be atomic.
    ///
    /// Both names must be in one directory because then the rename is one
    /// change to one directory, and one `sync_dir` makes it durable. A rename
    /// between two directories changes both. Each would need its own sync,
    /// and a crash between the two syncs could leave the file under both
    /// names or under neither. Nothing here needs to move a file between
    /// directories, so the trait does not allow it.
    ///
    /// # Errors
    ///
    /// - [`Error::CrossDirectory`](crate::Error::CrossDirectory) if the two
    ///   paths are in different directories.
    /// - [`Error::NotFound`](crate::Error::NotFound) if there is no `from`.
    /// - [`Error::WrongType`](crate::Error::WrongType) if `from` or `to` is
    ///   a directory.
    fn rename(&self, from: &Path, to: &Path) -> Result<()>;

    /// Creates an empty directory at `path`. Its parent must exist. The new
    /// directory is not durable until its parent is synced.
    ///
    /// # Errors
    ///
    /// - [`Error::NotFound`](crate::Error::NotFound) if the parent does not
    ///   exist.
    /// - [`Error::WrongType`](crate::Error::WrongType) if the parent is a
    ///   file.
    /// - [`Error::AlreadyExists`](crate::Error::AlreadyExists) if anything
    ///   is already at `path`.
    fn create_dir(&self, path: &Path) -> Result<()>;

    /// The paths of everything in the directory `dir`, files and
    /// directories, sorted. Each path includes `dir`, so it can be passed
    /// to `open` as it is.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`](crate::Error::NotFound) if there is no `dir`, and
    /// [`Error::WrongType`](crate::Error::WrongType) if it is a file.
    fn list(&self, dir: &Path) -> Result<Vec<PathBuf>>;

    /// Waits until the files created, removed and renamed in `dir` since the
    /// last `sync_dir` are on the disk. It does not sync the files' bytes.
    /// That is [`File::sync`].
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`](crate::Error::NotFound) if there is no `dir`.
    fn sync_dir(&self, dir: &Path) -> Result<()>;
}

/// One open file.
///
/// Every read and write names its offset, so there is no cursor, and threads
/// can share one handle. Writes are not durable until [`sync`](File::sync)
/// returns. Dropping the handle closes the file and releases its lock.
///
/// There is no cursor because a cursor is shared state. Two threads that
/// seek and then read on one handle can each move the cursor between the
/// other's seek and read. With the offset in every call, each call stands
/// alone, and every method takes `&self`.
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
/// let file = fs.open(Path::new("f"), Open::CreateNew).unwrap();
/// // Writing past the end fills the gap with zeros.
/// file.write_at(b"xy", 3).unwrap();
/// assert_eq!(file.len().unwrap(), 5);
/// let mut buf = [9; 5];
/// file.read_at(&mut buf, 0).unwrap();
/// assert_eq!(buf, [0, 0, 0, b'x', b'y']);
///
/// // A read has to get every byte it asks for.
/// assert!(matches!(file.read_at(&mut buf, 1), Err(Error::ShortRead { .. })));
/// # std::fs::remove_dir_all(&root).unwrap();
/// ```
pub trait File: Send + Sync {
    /// Fills `buf` with the bytes starting at `offset`. The read is all or
    /// nothing: if the file ends before `buf` is full, it fails.
    ///
    /// A database reads things of a known size, such as a page or a record
    /// header. If fewer bytes are there, the file is damaged or the caller
    /// has a bug, and either way the caller cannot use what it got. So a
    /// short read is an error, and no caller has to check a byte count.
    ///
    /// # Errors
    ///
    /// [`Error::ShortRead`](crate::Error::ShortRead) if the file ends
    /// before `offset + buf.len()`.
    fn read_at(&self, buf: &mut [u8], offset: u64) -> Result<()>;

    /// Writes all of `buf` starting at `offset`. Writing past the end makes
    /// the file longer, and any gap reads as zeros. A read sees the new
    /// bytes immediately, but they are not durable until `sync`.
    ///
    /// # Errors
    ///
    /// Any error from the file system. Part of `buf` may have been written
    /// then, so the bytes at that range are unknown.
    fn write_at(&self, buf: &[u8], offset: u64) -> Result<()>;

    /// The file's length, in bytes, including writes that are not synced yet.
    ///
    /// # Errors
    ///
    /// [`Error::Stale`](crate::Error::Stale) if a crash came after this
    /// handle was opened, or any error from the file system.
    fn len(&self) -> Result<u64>;

    /// Whether the file's length is 0.
    ///
    /// # Errors
    ///
    /// The same errors as [`File::len`].
    fn is_empty(&self) -> Result<bool> {
        Ok(self.len()? == 0)
    }

    /// Cuts the file to `len` bytes, or makes it longer with zeros. Like a
    /// write, it is not durable until `sync`.
    ///
    /// # Errors
    ///
    /// Any error from the file system. The length is then unknown.
    fn set_len(&self, len: u64) -> Result<()>;

    /// Waits until every write and `set_len` on this file so far is on the
    /// disk. After it returns `Ok`, a crash cannot lose them.
    ///
    /// # Errors
    ///
    /// Any error from the file system. On Linux, a failed sync may throw
    /// the unsynced writes away, and a later sync can still succeed. So
    /// after a failed sync, the caller cannot know what is on the disk, and
    /// should not just retry.
    fn sync(&self) -> Result<()>;

    /// Takes an exclusive lock on the file, so that a second handle, in this
    /// process or another, cannot take it. It never waits. The lock is
    /// released when this handle is dropped. Calling it again on the same
    /// handle succeeds.
    ///
    /// The lock is advisory: it does not stop reads or writes, only other
    /// `try_lock` calls. A caller takes it right after it opens a file, so
    /// that two processes never use the file at the same time.
    ///
    /// It never waits, because the other holder of the lock is usually a
    /// second copy of the same program, and that program may run for a long
    /// time. An immediate error tells the user what is wrong. A wait would
    /// look like the program had stopped working.
    ///
    /// # Errors
    ///
    /// [`Error::Locked`](crate::Error::Locked) if another handle holds the
    /// lock.
    fn try_lock(&self) -> Result<()>;
}
