//! Faults that a test can ask for: a write that fails, a write that stops
//! halfway, and a sync that fails.
//!
//! A failed sync is the most dangerous of the three. On Linux, `fsync`
//! returns the error, and the kernel drops the writes that were not saved.
//! Reads still return the new bytes from memory, but those bytes never
//! reach the disk, and the next `fsync` succeeds. `Mem` behaves the same
//! way.
//!
//! Faults wait in a queue in the order they were asked for. A write takes
//! the first write fault in the queue, and a sync takes the first sync
//! fault. A fault is used once. A crash clears the queue.
//!
//! A fault is aimed at a kind of call, not at a call number. A test usually
//! wants "the next sync fails", and it can ask for that without counting
//! how many writes come first. The writes before the sync do not use up the
//! sync fault.

use crate::{Error, Mem, Result, mem::State};

/// A fault [`Mem::fail_next`] can inject. The call it hits fails with
/// [`Error::Injected`] holding the fault.
///
/// # Examples
///
/// ```
/// use std::path::Path;
/// use vfs::{Fault, Mem, Open, Vfs};
///
/// let mem = Mem::new();
/// let file = mem.open(Path::new("a"), Open::CreateNew).unwrap();
/// mem.fail_next(Fault::ShortWrite);
/// assert!(file.write_at(b"abcd", 0).is_err());
/// // Half the bytes got through.
/// assert_eq!(file.len().unwrap(), 2);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// The next write fails and writes nothing. It still counts as a
    /// change.
    WriteError,
    /// The next write writes the first half of its bytes, rounded down,
    /// then fails. A caller that ignores the error has a half-written
    /// record.
    ShortWrite,
    /// The next file sync fails, and the writes it was saving are lost.
    /// Reads still return them until a crash, and a later sync succeeds
    /// without saving them. Directory syncs are not affected.
    SyncError,
}

impl Fault {
    /// The fault's name, for messages, such as `"sync error"`.
    ///
    /// # Examples
    ///
    /// ```
    /// use vfs::Fault;
    ///
    /// assert_eq!(Fault::SyncError.name(), "sync error");
    /// ```
    pub fn name(self) -> &'static str {
        match self {
            Fault::WriteError => "write error",
            Fault::ShortWrite => "short write",
            Fault::SyncError => "sync error",
        }
    }
}

/// Faults waiting to happen, in the order they were asked for.
#[derive(Default)]
pub(crate) struct Faults {
    waiting: Vec<Fault>,
}

impl Faults {
    /// Takes the first waiting fault that `applies` accepts out of the
    /// queue, or returns `None` if there is none.
    fn take(&mut self, applies: impl Fn(Fault) -> bool) -> Option<Fault> {
        let i = self.waiting.iter().position(|&f| applies(f))?;
        Some(self.waiting.remove(i))
    }
}

impl Mem {
    /// Makes the next write or file sync fail with `fault`. Call it more
    /// than once to queue several faults. A fault waits until a call of its
    /// kind comes, however many other calls come first.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::path::Path;
    /// use vfs::{Error, Fault, Mem, Open, Vfs};
    ///
    /// let mem = Mem::new();
    /// let file = mem.open(Path::new("a"), Open::CreateNew).unwrap();
    /// file.write_at(b"lost", 0).unwrap();
    /// mem.fail_next(Fault::SyncError);
    /// assert!(matches!(file.sync(), Err(Error::Injected(Fault::SyncError))));
    ///
    /// // The next sync succeeds, but the write it should have saved is gone.
    /// file.sync().unwrap();
    /// mem.sync_dir(Path::new("")).unwrap();
    /// mem.crash(0);
    /// let file = mem.open(Path::new("a"), Open::Existing).unwrap();
    /// assert_eq!(file.len().unwrap(), 0);
    /// ```
    pub fn fail_next(&self, fault: Fault) {
        self.lock().faults.waiting.push(fault);
    }
}

impl State {
    /// Uses up a waiting write fault, if there is one. Returns how many of the
    /// `len` bytes get written, and what the write returns. With no fault
    /// waiting, that is all `len` bytes and `Ok`.
    pub(crate) fn write_fault(&mut self, len: usize) -> (usize, Result<()>) {
        match self.faults.take(|f| f != Fault::SyncError) {
            None => (len, Ok(())),
            Some(Fault::ShortWrite) => (len / 2, Err(Error::Injected(Fault::ShortWrite))),
            Some(fault) => (0, Err(Error::Injected(fault))),
        }
    }

    /// Fails the sync if a `SyncError` is waiting, and drops the file's
    /// `unsynced` changes, as Linux does. `data` keeps them, so reads still see
    /// them until a crash.
    pub(crate) fn sync_fault(&mut self, inode: usize) -> Result<()> {
        if self.faults.take(|f| f == Fault::SyncError).is_some() {
            self.files[inode].unsynced.clear();
            return Err(Error::Injected(Fault::SyncError));
        }
        Ok(())
    }
}
