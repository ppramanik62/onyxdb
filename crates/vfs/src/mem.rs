//! `Mem`: a file system in memory.
//!
//! All files and directories are stored in one `State`, behind one mutex. A
//! clone of a `Mem` copies only a pointer to that state, and any thread can
//! use it.
//!
//! One mutex for everything is the simplest correct design. A crash must
//! see every file and directory at the same moment, and with one mutex it
//! only has to take that lock. The cost is that threads wait for each other
//! on every call. `Mem` is only for tests, so that cost does not matter.
//!
//! Every call behaves as it does on `Disk`, with the same errors. In
//! addition, `Mem` remembers which changes a real disk might not have saved
//! yet. The `crash` module uses this to decide what survives a power
//! failure.
//!
//! Each file and directory keeps three things: what it holds now, what it
//! held at the last sync, and the list of changes since that sync.
//!
//! The changes are kept as a list, and not only as their result, because a
//! crash treats each change separately. It can keep one write, lose the next
//! one, and tear a third one. To do that, the crash needs every write, in
//! order, with its offset.
//!
//! ```text
//! State
//!   files: [Inode]                 one per file ever created, by number
//!     data      what reads return now
//!     synced    what the last sync put on the disk
//!     unsynced  writes and set_lens since then, oldest first
//!   dirs: path -> Dir              the root directory is the empty path
//!     names     name -> file number or directory, as it is now
//!     synced    the names as of the last sync_dir
//!     unsynced  adds, removes and renames since then, oldest first
//! ```

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
};

use crate::{Error, File, Open, Result, Vfs, disk::check_path, fault::Faults};

/// A file system in memory. A clone is another handle to the same file system.
///
/// It starts empty, with only the root directory. Every [`Vfs`] and
/// [`File`] call works as on a real disk. `Mem` also keeps, for every file
/// and directory, what was synced and what was not. That is what lets a
/// test crash it at any point.
///
/// Every change counts toward the power cut that `power_off_after` sets. Once
/// the power is off, every change fails with `Error::PowerOff`. After a
/// crash, handles opened before it fail with [`Error::Stale`].
///
/// # Examples
///
/// ```
/// use std::path::Path;
/// use vfs::{Mem, Open, Vfs};
///
/// let mem = Mem::new();
/// let file = mem.open(Path::new("a.db"), Open::CreateNew).unwrap();
/// file.write_at(b"hello", 0).unwrap();
///
/// // A clone sees the same files.
/// let other = mem.clone();
/// let file = other.open(Path::new("a.db"), Open::Existing).unwrap();
/// let mut buf = [0; 5];
/// file.read_at(&mut buf, 0).unwrap();
/// assert_eq!(&buf, b"hello");
/// ```
#[derive(Clone)]
pub struct Mem {
    state: Arc<Mutex<State>>,
}

/// Everything a `Mem` holds.
///
/// It and the types below are `pub(crate)`. Every module in this crate can
/// see them, because `crash` and `fault` change them. Code outside the
/// crate cannot.
pub(crate) struct State {
    /// Starts at 0 and goes up by one at every crash. Each file handle stores
    /// the generation it was opened in.
    pub(crate) generation: u64,
    /// Every file ever created. A file's number is its index here. Removing a
    /// file only removes its name, so handles that are open keep working.
    ///
    /// Files are never taken out of this list, so a number is never reused.
    /// That way a handle's number cannot end up pointing at a different file
    /// created later. The cost is that a removed file's bytes stay in memory
    /// until the `Mem` is dropped, which is fine for a test.
    pub(crate) files: Vec<Inode>,
    /// Every directory, by path. The root directory is the empty path.
    pub(crate) dirs: BTreeMap<PathBuf, Dir>,
    /// How many changes have been made: writes, set_lens, syncs, creates,
    /// removes, renames and directory syncs. Reads are not changes.
    pub(crate) changes: u64,
    /// How many more changes can happen before the power goes off. `None` means
    /// no limit.
    pub(crate) power_left: Option<u64>,
    /// Faults waiting to happen.
    pub(crate) faults: Faults,
    /// The files whose lock some open handle holds, by number.
    pub(crate) locked: BTreeSet<usize>,
}

/// The contents of one file.
#[derive(Default)]
pub(crate) struct Inode {
    /// What reads return now.
    pub(crate) data: Vec<u8>,
    /// What the file held at its last successful sync.
    pub(crate) synced: Vec<u8>,
    /// Changes since the last successful sync, oldest first.
    pub(crate) unsynced: Vec<Change>,
}

/// One change to a file's contents.
pub(crate) enum Change {
    /// `bytes` written at `offset`.
    Write {
        /// Where the write started, in bytes.
        offset: u64,
        /// What it wrote.
        bytes: Vec<u8>,
    },
    /// The file's length set to this many bytes.
    SetLen(u64),
}

/// The names in one directory.
#[derive(Default)]
pub(crate) struct Dir {
    /// Each name and what it refers to, as it is now.
    pub(crate) names: BTreeMap<OsString, Node>,
    /// The names as of the last `sync_dir`.
    pub(crate) synced: BTreeMap<OsString, Node>,
    /// Name changes since the last `sync_dir`, oldest first.
    pub(crate) unsynced: Vec<NameChange>,
}

/// What a name in a directory refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Node {
    /// The file with this number in `State::files`.
    File(usize),
    /// A directory, found in `State::dirs` by its path.
    Dir,
}

/// One change to the names in a directory.
pub(crate) enum NameChange {
    /// A new name.
    Add(OsString, Node),
    /// A name taken away.
    Remove(OsString),
    /// A name changed, replacing whatever had the new name.
    Rename(OsString, OsString),
}

impl NameChange {
    /// Makes this change to `names`. A rename of a name that is not there
    /// does nothing.
    pub(crate) fn apply(&self, names: &mut BTreeMap<OsString, Node>) {
        match self {
            NameChange::Add(name, node) => {
                names.insert(name.clone(), *node);
            }
            NameChange::Remove(name) => {
                names.remove(name);
            }
            NameChange::Rename(from, to) => {
                if let Some(node) = names.remove(from) {
                    names.insert(to.clone(), node);
                }
            }
        }
    }
}

impl Dir {
    /// Makes `change` to the names now, and remembers it as not yet synced.
    fn change(&mut self, change: NameChange) {
        change.apply(&mut self.names);
        self.unsynced.push(change);
    }
}

impl Change {
    /// Makes this change to `data`.
    pub(crate) fn apply(&self, data: &mut Vec<u8>) {
        match self {
            Change::Write { offset, bytes } => write_into(data, *offset, bytes),
            Change::SetLen(len) => data.resize(*len as usize, 0),
        }
    }
}

impl Inode {
    /// Writes `bytes` at `offset`, filling any gap before it with zeros, and
    /// remembers the write as not yet synced.
    fn write(&mut self, offset: u64, bytes: &[u8]) {
        write_into(&mut self.data, offset, bytes);
        self.unsynced.push(Change::Write {
            offset,
            bytes: bytes.to_vec(),
        });
    }
}

/// Copies `bytes` into `data` at `offset`, making `data` longer with zeros if
/// needed.
pub(crate) fn write_into(data: &mut Vec<u8>, offset: u64, bytes: &[u8]) {
    let start = offset as usize;
    let end = start + bytes.len();
    if data.len() < end {
        data.resize(end, 0);
    }
    data[start..end].copy_from_slice(bytes);
}

impl Mem {
    /// Makes an empty file system: just the root directory, with the power
    /// on and no faults waiting.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::path::Path;
    /// use vfs::{Mem, Vfs};
    ///
    /// let mem = Mem::new();
    /// assert!(mem.list(Path::new("")).unwrap().is_empty());
    /// ```
    pub fn new() -> Mem {
        let mut dirs = BTreeMap::new();
        dirs.insert(PathBuf::new(), Dir::default());
        let state = State {
            generation: 0,
            files: Vec::new(),
            dirs,
            changes: 0,
            power_left: None,
            faults: Faults::default(),
            locked: BTreeSet::new(),
        };
        Mem {
            state: Arc::new(Mutex::new(state)),
        }
    }

    /// Locks the state. If a thread panicked while holding the lock, the lock
    /// is still taken: a test still wants to read the state.
    pub(crate) fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Default for Mem {
    fn default() -> Self {
        Mem::new()
    }
}

/// Splits `path` into its directory and its last name. `a/b/c` gives `a/b`
/// and `c`, and `c` gives the root, the empty path, and `c`.
///
/// # Errors
///
/// [`Error::BadPath`] if `path` is not made of plain names, or is empty.
fn split(path: &Path) -> Result<(PathBuf, OsString)> {
    check_path(path)?;
    let name = path
        .file_name()
        .ok_or_else(|| Error::BadPath(path.to_path_buf()))?;
    let dir = path.parent().unwrap_or(Path::new("")).to_path_buf();
    Ok((dir, name.to_os_string()))
}

impl State {
    /// The directory at `path`, to read or change its names.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] if no directory has that path.
    fn dir(&mut self, path: &Path) -> Result<&mut Dir> {
        self.dirs
            .get_mut(path)
            .ok_or_else(|| Error::NotFound(path.to_path_buf()))
    }

    /// What `path` refers to: a file, a directory, or `None` if nothing is
    /// there or its directory does not exist.
    ///
    /// # Errors
    ///
    /// [`Error::BadPath`] if `path` is not made of plain names.
    fn node(&mut self, path: &Path) -> Result<Option<Node>> {
        if path.as_os_str().is_empty() {
            return Ok(Some(Node::Dir));
        }
        let (dir, name) = split(path)?;
        Ok(self
            .dirs
            .get(&dir)
            .and_then(|d| d.names.get(&name).copied()))
    }
}

impl Vfs for Mem {
    fn open(&self, path: &Path, mode: Open) -> Result<Box<dyn File>> {
        let (dir, name) = split(path)?;
        let mut state = self.lock();
        let found = state.dir(&dir)?.names.get(&name).copied();
        let inode = match (found, mode) {
            (Some(Node::File(inode)), Open::Existing) => inode,
            (Some(Node::Dir), Open::Existing) => return Err(Error::WrongType(path.to_path_buf())),
            (Some(_), Open::CreateNew) => return Err(Error::AlreadyExists(path.to_path_buf())),
            (None, Open::Existing) => return Err(Error::NotFound(path.to_path_buf())),
            (None, Open::CreateNew) => {
                state.change()?;
                let inode = state.files.len();
                state.files.push(Inode::default());
                state
                    .dir(&dir)?
                    .change(NameChange::Add(name, Node::File(inode)));
                inode
            }
        };
        let generation = state.generation;
        let holds_lock = AtomicBool::new(false);
        Ok(Box::new(MemFile {
            mem: self.clone(),
            generation,
            inode,
            path: path.to_path_buf(),
            holds_lock,
        }))
    }

    fn exists(&self, path: &Path) -> Result<bool> {
        Ok(self.lock().node(path)?.is_some())
    }

    fn remove(&self, path: &Path) -> Result<()> {
        let (dir, name) = split(path)?;
        let mut state = self.lock();
        match state.node(path)? {
            Some(Node::File(_)) => {}
            Some(Node::Dir) => return Err(Error::WrongType(path.to_path_buf())),
            None => return Err(Error::NotFound(path.to_path_buf())),
        }
        state.change()?;
        state.dir(&dir)?.change(NameChange::Remove(name));
        Ok(())
    }

    fn rename(&self, from: &Path, to: &Path) -> Result<()> {
        let ((dir, from_name), (to_dir, to_name)) = (split(from)?, split(to)?);
        if dir != to_dir {
            return Err(Error::CrossDirectory {
                from: from.to_path_buf(),
                to: to.to_path_buf(),
            });
        }
        let mut state = self.lock();
        match (state.node(from)?, state.node(to)?) {
            (None, _) => return Err(Error::NotFound(from.to_path_buf())),
            (Some(Node::Dir), _) | (_, Some(Node::Dir)) => {
                return Err(Error::WrongType(from.to_path_buf()));
            }
            _ => {}
        }
        state.change()?;
        state
            .dir(&dir)?
            .change(NameChange::Rename(from_name, to_name));
        Ok(())
    }

    fn create_dir(&self, path: &Path) -> Result<()> {
        let (dir, name) = split(path)?;
        let mut state = self.lock();
        match state.node(&dir)? {
            Some(Node::Dir) => {}
            Some(Node::File(_)) => return Err(Error::WrongType(dir)),
            None => return Err(Error::NotFound(dir)),
        }
        if state.node(path)?.is_some() {
            return Err(Error::AlreadyExists(path.to_path_buf()));
        }
        state.change()?;
        state.dir(&dir)?.change(NameChange::Add(name, Node::Dir));
        state.dirs.insert(path.to_path_buf(), Dir::default());
        Ok(())
    }

    fn list(&self, dir: &Path) -> Result<Vec<PathBuf>> {
        check_path(dir)?;
        let mut state = self.lock();
        if let Some(Node::File(_)) = state.node(dir)? {
            return Err(Error::WrongType(dir.to_path_buf()));
        }
        Ok(state
            .dir(dir)?
            .names
            .keys()
            .map(|name| dir.join(name))
            .collect())
    }

    fn sync_dir(&self, dir: &Path) -> Result<()> {
        check_path(dir)?;
        let mut state = self.lock();
        state.dir(dir)?;
        state.change()?;
        let d = state.dir(dir)?;
        d.synced = d.names.clone();
        d.unsynced.clear();
        Ok(())
    }
}

/// A file opened by `Mem`. It refers to its file by number, so a rename or
/// remove of its name does not affect it. That is how a file handle behaves on
/// Unix, and code tested on `Mem` must see what it would see on `Disk`.
struct MemFile {
    mem: Mem,
    /// The generation the file was opened in.
    generation: u64,
    /// The file's number in `State::files`.
    inode: usize,
    /// The path it was opened with, for error messages.
    path: PathBuf,
    /// Whether this handle holds the file's lock.
    ///
    /// It is an `AtomicBool` because every `File` method takes `&self`, so a
    /// plain `bool` could not be changed. `Relaxed` ordering is enough. It is
    /// only changed while the state's mutex is held, and the mutex orders
    /// those changes. `drop` reads it without the mutex, but by then no other
    /// thread can use the handle.
    holds_lock: AtomicBool,
}

impl MemFile {
    /// Locks the state, after checking that no crash happened since the file
    /// was opened.
    ///
    /// # Errors
    ///
    /// [`Error::Stale`] if there was a crash since then.
    fn state(&self) -> Result<MutexGuard<'_, State>> {
        let state = self.mem.lock();
        if state.generation != self.generation {
            return Err(Error::Stale(self.path.clone()));
        }
        Ok(state)
    }
}

impl File for MemFile {
    fn read_at(&self, buf: &mut [u8], offset: u64) -> Result<()> {
        let state = self.state()?;
        let data = &state.files[self.inode].data;
        let file_len = data.len() as u64;
        let end = offset
            .checked_add(buf.len() as u64)
            .filter(|&end| end <= file_len);
        if end.is_none() {
            return Err(Error::ShortRead {
                path: self.path.clone(),
                offset,
                len: buf.len(),
                file_len,
            });
        }
        let start = offset as usize;
        buf.copy_from_slice(&data[start..start + buf.len()]);
        Ok(())
    }

    fn write_at(&self, buf: &[u8], offset: u64) -> Result<()> {
        let mut state = self.state()?;
        state.change()?;
        // A fault may let only part of the write through, or none of it.
        let (written, result) = state.write_fault(buf.len());
        state.files[self.inode].write(offset, &buf[..written]);
        result
    }

    fn len(&self) -> Result<u64> {
        Ok(self.state()?.files[self.inode].data.len() as u64)
    }

    fn set_len(&self, len: u64) -> Result<()> {
        let mut state = self.state()?;
        state.change()?;
        let inode = &mut state.files[self.inode];
        inode.data.resize(len as usize, 0);
        inode.unsynced.push(Change::SetLen(len));
        Ok(())
    }

    fn sync(&self) -> Result<()> {
        let mut state = self.state()?;
        state.change()?;
        state.sync_fault(self.inode)?;
        // Bring `synced` up to date by applying `unsynced` to it. Copying
        // `data` would be wrong. A failed sync empties `unsynced` but leaves
        // `data` alone, so `data` can hold writes that will never reach the
        // disk. Those must not become synced now.
        let inode = &mut state.files[self.inode];
        for change in inode.unsynced.drain(..) {
            change.apply(&mut inode.synced);
        }
        Ok(())
    }

    fn try_lock(&self) -> Result<()> {
        let mut state = self.state()?;
        if self.holds_lock.load(Ordering::Relaxed) {
            return Ok(());
        }
        if !state.locked.insert(self.inode) {
            return Err(Error::Locked(self.path.clone()));
        }
        self.holds_lock.store(true, Ordering::Relaxed);
        Ok(())
    }
}

impl Drop for MemFile {
    /// Releases the lock, like closing a real file does. If a crash happened
    /// since the file was opened, there is nothing to release: the crash
    /// already released every lock.
    fn drop(&mut self) {
        if self.holds_lock.load(Ordering::Relaxed) {
            let mut state = self.mem.lock();
            if state.generation == self.generation {
                state.locked.remove(&self.inode);
            }
        }
    }
}
