//! `Wal`: appending records, making them durable, and opening the log again.
//!
//! | Term | Meaning |
//! |---|---|
//! | end | The LSN just past the last record appended. |
//! | durable | The LSN up to which every record is known to be on disk. |
//! | current segment | The newest segment, the one appends go to. |
//! | stopped | The state after a failed write, sync, create or remove. Every call fails until the log is opened again. |
//!
//! ```text
//!  append   ──▶ write_at into the current segment     (the OS's page cache)
//!  flush_to ──▶ sync the current segment              (the disk)
//!
//!  0                   durable                   end
//!  ├──────────────────────┼───────────────────────┤
//!                      on disk         written, a crash may lose it
//! ```
//!
//! `append` writes the record at once. The operating system keeps the bytes
//! in its page cache, which is already a buffer, so the log doesn't keep one
//! of its own. A record is on disk only after `flush_to` syncs it, and a
//! caller should report a change as saved only after that.
//!
//! One mutex covers the whole state. It's held while writing, so records go
//! into the file in the order their LSNs were handed out. It's also held
//! while syncing, which is the simple choice: an append waits for a sync
//! that's running, so every commit pays for a whole sync of its own.
//!
//! When the current segment is full, it's synced before the next one is
//! created, and the new file's name is synced into the directory before any
//! record goes in it. Otherwise a later segment could hold records on disk
//! while an earlier one lost its tail in a crash. Replay would stop at that
//! hole, and the later records would be lost although they were synced.
//!
//! On Linux, a failed `fsync` throws away the writes it was saving and marks
//! them clean, so the next `fsync` succeeds without them. A failed write may
//! have written part of a record. Either way, the log no longer knows what's
//! on disk. So after any failed write, sync, create or remove, it stops:
//! every call returns `Error::Stopped` until the log is opened again.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

use vfs::{File, Open, Vfs};

use crate::{BLOCK, Error, Lsn, Result, format::encode, segment};

/// An append-only log of records, stored in segment files in one directory.
///
/// A log is made once with [`create`](Wal::create), and opened with
/// [`open`](Wal::open) every time after that. `Wal` is `Send` and `Sync`, so
/// threads can share one through an `Arc` or a reference.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use vfs::Mem;
/// use wal::{BLOCK, Wal};
///
/// let wal = Wal::create(Arc::new(Mem::new()), "wal", BLOCK as u64).unwrap();
/// let lsn = wal.append(b"put apple 1").unwrap();
/// wal.flush_to(lsn).unwrap();
/// assert_eq!(wal.durable(), lsn);
/// ```
pub struct Wal {
    /// The file system the segments live in. `dyn Vfs` is any type that
    /// implements `Vfs`, and the `Arc` lets the caller keep using it too.
    vfs: Arc<dyn Vfs>,
    /// The directory that holds the segments.
    dir: PathBuf,
    /// How many bytes a segment holds before the next one starts. A positive
    /// multiple of `BLOCK`, so a block never spans two files.
    segment_size: u64,
    /// Everything that changes, behind one mutex.
    state: Mutex<State>,
}

/// The part of a [`Wal`] that changes as records are appended.
struct State {
    /// The start of every segment, oldest first. The last one is the
    /// current segment.
    segments: Vec<Lsn>,
    /// The current segment's file.
    file: Box<dyn File>,
    /// The LSN just past the last record appended.
    end: Lsn,
    /// Up to this LSN the records are on disk. Never more than `end` and
    /// never goes down.
    durable: Lsn,
    /// Set after a failed write, sync, create or remove. While it's set,
    /// every call returns `Error::Stopped`
    stopped: bool,
}

impl Wal {
    /// Creates a new, empty log in `dir`, which must not exist yet.
    ///
    /// Makes `dir`, syncs its parent so the new directory is on disk, and
    /// creates the first segment, which starts at LSN 0. Each segment will
    /// hold `segment_size` bytes before the next one starts.
    ///
    /// # Errors
    ///
    /// [`Error::Vfs`] if a file system call fails. If `dir` already exists,
    /// that's `vfs::Error::AlreadyExists`.
    ///
    /// # Panics
    ///
    /// If `segment_size` is 0 or not a multiple of [`BLOCK`].
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use vfs::Mem;
    /// use wal::{BLOCK, Wal};
    ///
    /// let mem = Mem::new();
    /// let wal = Wal::create(Arc::new(mem.clone()), "wal", 4 * BLOCK as u64).unwrap();
    /// assert_eq!(wal.end(), 0);
    /// // A second log in the same place is refused.
    /// assert!(Wal::create(Arc::new(mem), "wal", 4 * BLOCK as u64).is_err());
    /// ```
    pub fn create(vfs: Arc<dyn Vfs>, dir: impl AsRef<Path>, segment_size: u64) -> Result<Wal> {
        check_size(segment_size);
        let dir = dir.as_ref();
        vfs.create_dir(dir)?;
        vfs.sync_dir(dir.parent().unwrap_or(Path::new("")))?;
        let file = new_segment(&*vfs, dir, 0)?;
        Ok(Wal::new(vfs, dir, segment_size, vec![0], file, 0))
    }

    /// A `Wal` whose segments are `segments`, with `file` open as the
    /// current one, and every record up to `end` on disk.
    fn new(
        vfs: Arc<dyn Vfs>,
        dir: &Path,
        segment_size: u64,
        segments: Vec<Lsn>,
        file: Box<dyn File>,
        end: Lsn,
    ) -> Wal {
        let state = State {
            segments,
            file,
            end,
            durable: end,
            stopped: false,
        };
        Wal {
            vfs,
            dir: dir.to_path_buf(),
            segment_size,
            state: Mutex::new(state),
        }
    }
}

/// Panics unless `segment_size` is a positive multiple of `BLOCK`.
fn check_size(segment_size: u64) {
    assert!(
        segment_size > 0 && segment_size.is_multiple_of(BLOCK as u64),
        "a segment must hold a whole number of {BLOCK}-byte blocks, but segment size is {segment_size}"
    )
}

/// Creates the empty segment that starts at `start`, and syncs `dir`, so
/// the new name is on disk before any record is written in the file.
fn new_segment(vfs: &dyn Vfs, dir: &Path, start: Lsn) -> Result<Box<dyn File>> {
    let file = vfs.open(&segment::path(dir, start), Open::CreateNew)?;
    vfs.sync_dir(dir)?;
    Ok(file)
}

impl Wal {
    /// Appends `record` to the log, and returns its LSN.
    ///
    /// The record is written to the current segment at once, and goes on in
    /// a new segment if it fills this one. It isn't durable yet: call
    /// [`flush_to`](Wal::flush_to) with the LSN before telling anyone it's
    /// saved. A record can be any length, including 0. Replay returns it
    /// whole or not at all.
    ///
    /// The LSN is the log position just past the record. It's bigger than
    /// every LSN returned before, and [`end`](Wal::end) returns it until the
    /// next append.
    ///
    /// # Errors
    ///
    /// - [`Error::Stopped`] if the log has stopped.
    /// - [`Error::Vfs`] if a write fails, or if syncing a full segment or
    ///   creating the next one fails. The log then stops.
    ///
    /// # Panics
    ///
    /// If another thread panicked while it held the log's mutex.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use vfs::Mem;
    /// use wal::{BLOCK, HEADER, Wal};
    ///
    /// let wal = Wal::create(Arc::new(Mem::new()), "wal", BLOCK as u64).unwrap();
    /// // A 10-byte record is a 7-byte header and its data.
    /// assert_eq!(wal.append(b"0123456789").unwrap(), (HEADER + 10) as u64);
    /// assert_eq!(wal.append(b"").unwrap(), (HEADER + 10 + HEADER) as u64);
    /// ```
    pub fn append(&self, record: &[u8]) -> Result<Lsn> {
        let mut state = self.lock()?;
        let result = self.write_record(&mut state, record);
        if result.is_err() {
            state.stopped = true;
        }
        result
    }

    /// Writes `record`'s bytes at the end of the log, starting a new segment
    /// each time the current one is full, and moves `end` past them.
    fn write_record(&self, state: &mut State, record: &[u8]) -> Result<Lsn> {
        let bytes = encode(state.end, record);
        let mut pos = state.end;
        let mut rest = &bytes[..];
        while !rest.is_empty() {
            let start = *state.segments.last().expect("a log always has a segment");
            // Normally a segment ends `segment_size` bytes after its start. If
            // the log was opened with a smaller size than it was written
            // with, this segment may already be longer, and it ends at the
            // next block boundary instead.
            let full = (start + self.segment_size).max(pos.next_multiple_of(BLOCK as u64));
            if pos == full {
                self.next_segment(state, pos)?;
                continue;
            }
            // Write what fits in this segment. The rest goes in the next one.
            let n = rest.len().min((full - pos) as usize);
            state.file.write_at(&rest[..n], pos - start)?;
            pos += n as u64;
            rest = &rest[n..];
        }
        state.end = pos;
        Ok(pos)
    }

    /// Syncs the full current segment, then creates the segment that starts
    /// at `start` and makes it the current one.
    fn next_segment(&self, state: &mut State, start: Lsn) -> Result<()> {
        // Every earlier segment must be on disk before a later one can hold
        // a durable record.
        state.file.sync()?;
        state.file = new_segment(&*self.vfs, &self.dir, start)?;
        state.segments.push(start);
        Ok(())
    }

    /// Makes every record whose LSN is at most `lsn` durable, and returns
    /// once they're on disk.
    ///
    /// When `lsn` is past the end, this makes the whole log durable, so
    /// `flush_to(u64::MAX)` means "everything appended so far". When those
    /// records are already durable, it returns at once without syncing.
    /// Otherwise it syncs the current segment, which makes every record
    /// appended so far durable, not only the ones up to `lsn`.
    ///
    /// Afterwards, [`durable`](Wal::durable) is at least the smaller of
    /// `lsn` and [`end`](Wal::end).
    ///
    /// # Errors
    ///
    /// - [`Error::Stopped`] if the log has stopped.
    /// - [`Error::Vfs`] if the sync fails. The records that weren't durable
    ///   may be lost, so don't report them as saved. The log then stops.
    ///
    /// # Panics
    ///
    /// If another thread panicked while it held the log's mutex.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use vfs::Mem;
    /// use wal::{BLOCK, Wal};
    ///
    /// let wal = Wal::create(Arc::new(Mem::new()), "wal", BLOCK as u64).unwrap();
    /// let first = wal.append(b"first").unwrap();
    /// let second = wal.append(b"second").unwrap();
    /// assert_eq!(wal.durable(), 0);
    ///
    /// // One sync makes both records durable.
    /// wal.flush_to(first).unwrap();
    /// assert_eq!(wal.durable(), second);
    /// ```
    pub fn flush_to(&self, lsn: Lsn) -> Result<()> {
        let mut state = self.lock()?;
        if state.durable >= lsn.min(state.end) {
            return Ok(());
        }
        if let Err(e) = state.file.sync() {
            state.stopped = true;
            return Err(e.into());
        }
        // Earlier segments were synced when they filled up, so the whole log
        // is on disk now.
        state.durable = state.end;
        Ok(())
    }

    /// The LSN just past the last record appended. Before any append, it's
    /// where the log's records end, which is 0 for a new log.
    ///
    /// # Panics
    ///
    /// If another thread panicked while it held the log's mutex.
    pub fn end(&self) -> Lsn {
        self.state().end
    }

    /// The LSN up to which every record is on disk. It never goes down, and
    /// it's never past [`end`](Wal::end).
    ///
    /// # Panics
    ///
    /// If another thread panicked while it held the log's mutex.
    pub fn durable(&self) -> Lsn {
        self.state().durable
    }

    /// Locks the state, whether or not the log has stopped.
    ///
    /// # Panics
    ///
    /// If another thread panicked while it held the mutex, because `end`
    /// and the current segment may then disagree.
    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .expect("a thread panicked while holding log's mutex")
    }

    /// Locks the state, or returns `Error::Stopped` if the log has stopped.
    fn lock(&self) -> Result<MutexGuard<'_, State>> {
        let state = self.state();
        if state.stopped {
            return Err(Error::Stopped);
        }
        Ok(state)
    }
}
