//! `Disk`: the real file system, through `std::fs`.
//!
//! Every path is joined to a root directory first, so `Disk::new("data")`
//! and the path `pages.pf` mean the file `data/pages.pf`. Reads and writes
//! use positioned I/O from `std::os::unix::fs::FileExt`, so this module is
//! Unix-only.

use std::{
    fs::{self, OpenOptions},
    io,
    os::unix::fs::FileExt,
    path::{Component, Path, PathBuf},
};

use crate::{Error, File, Open, Result, Vfs};

/// The real file system, below one root directory.
///
/// Each call goes directly to the operating system. `Disk` has no cache of
/// its own. `sync` is `fsync` on the file, and `sync_dir` is `fsync` on the
/// directory.
///
/// It has no cache because the operating system already keeps recently
/// used file data in memory. A second cache here would use more memory. It
/// would also hide which writes the operating system has received. Code
/// that needs its own cache builds it above `Disk`, where it knows which
/// data is worth keeping.
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
/// fs.create_dir(Path::new("data")).unwrap();
/// fs.open(Path::new("data/a.db"), Open::CreateNew).unwrap();
/// assert!(root.join("data/a.db").exists());
///
/// // Paths can't climb out of the root.
/// assert!(matches!(fs.exists(Path::new("../x")), Err(Error::BadPath(_))));
/// # std::fs::remove_dir_all(&root).unwrap();
/// ```
pub struct Disk {
    root: PathBuf,
}

impl Disk {
    /// A file system whose paths are relative to `root`. `root` must exist.
    /// Nothing is checked or opened until the first call.
    ///
    /// `root` takes `impl Into<PathBuf>`, so a `&str`, a `&Path` or a
    /// `PathBuf` all work.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::path::Path;
    /// use vfs::{Disk, Error, Open, Vfs};
    ///
    /// // Making it checks nothing, so a missing root shows up at the first call.
    /// let fs = Disk::new("/no/such/root");
    /// let result = fs.open(Path::new("a.db"), Open::Existing);
    /// assert!(matches!(result, Err(Error::NotFound(_))));
    /// ```
    pub fn new(root: impl Into<PathBuf>) -> Disk {
        Disk { root: root.into() }
    }

    /// The real path of `path`: `root` joined with it, after checking that
    /// `path` is made only of plain names.
    fn real(&self, path: &Path) -> Result<PathBuf> {
        check_path(path)?;
        Ok(self.root.join(path))
    }
}

/// Checks that `path` is relative and made only of plain names, so it cannot
/// reach outside the root. The empty path, the root, passes.
///
/// # Errors
///
/// [`Error::BadPath`] if `path` is absolute or has `.` or `..` in it.
///
/// # Examples
///
/// ```
/// use std::path::Path;
/// use vfs::disk::check_path;
///
/// assert!(check_path(Path::new("data/a.db")).is_ok());
/// assert!(check_path(Path::new("/etc/passwd")).is_err());
/// assert!(check_path(Path::new("data/../a.db")).is_err());
/// ```
pub fn check_path(path: &Path) -> Result<()> {
    if path.components().all(|c| matches!(c, Component::Normal(_))) {
        Ok(())
    } else {
        Err(Error::BadPath(path.to_path_buf()))
    }
}

/// The real file system. Each method is the matching `std::fs` call on the
/// path joined to the root, with its error turned into [`Error`].
///
/// Some methods check the path before they call the operating system. The
/// operating system accepts a directory in `open`, `remove` and `rename`,
/// but the trait says these calls must fail on a directory. So `Disk` checks
/// for a directory first. When `rename` fails for this reason, the error
/// names `from`, even if `to` is the directory.
impl Vfs for Disk {
    fn open(&self, path: &Path, mode: crate::Open) -> Result<Box<dyn File>> {
        let real = self.real(path)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true);
        if mode == Open::CreateNew {
            options.create_new(true);
        }
        let file = options
            .open(&real)
            .map_err(|e| Error::from_io("open", path, e))?;
        // Opening a directory succeeds on Unix; using it as a file does not.
        let is_dir = file
            .metadata()
            .map_err(|e| Error::from_io("open", path, e))?
            .is_dir();
        if is_dir {
            return Err(Error::WrongType(path.to_path_buf()));
        }
        Ok(Box::new(DiskFile {
            file,
            path: path.to_path_buf(),
        }))
    }

    fn exists(&self, path: &Path) -> Result<bool> {
        let real = self.real(path)?;
        real.try_exists()
            .map_err(|e| Error::from_io("exists", path, e))
    }

    fn remove(&self, path: &Path) -> Result<()> {
        let real = self.real(path)?;
        if real.is_dir() {
            return Err(Error::WrongType(path.to_path_buf()));
        }
        fs::remove_file(real).map_err(|e| Error::from_io("remove", path, e))
    }

    fn rename(&self, from: &Path, to: &Path) -> Result<()> {
        if from.parent() != to.parent() {
            return Err(Error::CrossDirectory {
                from: from.to_path_buf(),
                to: to.to_path_buf(),
            });
        }
        let (real_from, real_to) = (self.real(from)?, self.real(to)?);
        if real_from.is_dir() || real_to.is_dir() {
            return Err(Error::WrongType(from.to_path_buf()));
        }
        fs::rename(real_from, real_to).map_err(|e| Error::from_io("rename", from, e))
    }

    fn create_dir(&self, path: &Path) -> Result<()> {
        let real = self.real(path)?;
        fs::create_dir(real).map_err(|e| Error::from_io("create_dir", path, e))
    }

    fn list(&self, dir: &Path) -> Result<Vec<PathBuf>> {
        let real = self.real(dir)?;
        let entries = fs::read_dir(real).map_err(|e| Error::from_io("list", dir, e))?;
        let mut paths = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| Error::from_io("list", dir, e))?;
            paths.push(dir.join(entry.file_name()));
        }
        paths.sort();
        Ok(paths)
    }

    fn sync_dir(&self, dir: &Path) -> Result<()> {
        let real = self.real(dir)?;
        // On Linux a directory is synced by opening it and syncing the handle.
        let handle = fs::File::open(real).map_err(|e| Error::from_io("sync_dir", dir, e))?;
        handle
            .sync_all()
            .map_err(|e| Error::from_io("sync_dir", dir, e))
    }
}

/// A file opened by `Disk`: the open `std::fs::File`, and the path it was
/// opened with, for error messages.
struct DiskFile {
    file: fs::File,
    path: PathBuf,
}

impl File for DiskFile {
    fn read_at(&self, buf: &mut [u8], offset: u64) -> Result<()> {
        match self.file.read_exact_at(buf, offset) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Err(Error::ShortRead {
                path: self.path.clone(),
                offset,
                len: buf.len(),
                file_len: self.len()?,
            }),
            Err(e) => Err(Error::from_io("read", &self.path, e)),
        }
    }

    fn write_at(&self, buf: &[u8], offset: u64) -> Result<()> {
        self.file
            .write_all_at(buf, offset)
            .map_err(|e| Error::from_io("write", &self.path, e))
    }

    fn len(&self) -> Result<u64> {
        let meta = self
            .file
            .metadata()
            .map_err(|e| Error::from_io("len", &self.path, e))?;
        Ok(meta.len())
    }

    fn set_len(&self, len: u64) -> Result<()> {
        self.file
            .set_len(len)
            .map_err(|e| Error::from_io("set_len", &self.path, e))
    }

    fn sync(&self) -> Result<()> {
        self.file
            .sync_all()
            .map_err(|e| Error::from_io("sync", &self.path, e))
    }

    fn try_lock(&self) -> Result<()> {
        // This is `flock`: the lock belongs to this open file, and closing the
        // file releases it.
        match self.file.try_lock() {
            Ok(()) => Ok(()),
            Err(fs::TryLockError::WouldBlock) => Err(Error::Locked(self.path.clone())),
            Err(fs::TryLockError::Error(e)) => Err(Error::from_io("lock", &self.path, e)),
        }
    }
}
