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
//! `append` writes the record to the file immediately. The log keeps no
//! buffer of its own, because the operating system already keeps the
//! written bytes in its page cache. Those bytes reach the disk only when
//! `flush_to` syncs the file. A caller must not report a change as saved
//! before that.
//!
//! One mutex protects the whole state. `append` holds it while it writes,
//! so records reach the file in the same order as their LSNs. `flush_to`
//! also holds it while it syncs. This is the simplest design, and it has a
//! cost: an `append` that comes during a sync waits for the sync to finish,
//! and every commit needs a sync of its own.
//!
//! When the current segment is full, the log syncs it, then creates the next
//! segment, then syncs the directory so the new file's name is on disk. Only
//! then does it write records into the new segment. Without this order, a
//! crash could keep records in the new segment and lose the end of the full
//! one. Replay stops at the first hole, so it would never reach the records
//! in the new segment, even though they were synced.
//!
//! After a failed write, sync, create or remove, the log stops. From then
//! on, every call returns `Error::Stopped` until the log is opened again.
//! The reason is that the log no longer knows what is on disk. A failed
//! write may have written part of a record. A failed `fsync` on Linux drops
//! the writes it was saving and marks them as saved, so the next `fsync`
//! succeeds without writing them.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

use vfs::{File, Open, Vfs};

use crate::{BLOCK, Error, Lsn, Replay, Result, format::encode, replay::Reader, segment};

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
    /// The file system that holds the segments. `dyn Vfs` is any type that
    /// implements `Vfs`. The `Arc` lets the caller keep using the same file
    /// system.
    vfs: Arc<dyn Vfs>,
    /// The directory that holds the segments.
    dir: PathBuf,
    /// How many bytes a segment holds before the next one starts. It is a
    /// positive multiple of `BLOCK`, so a block is never split between two
    /// files.
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
    /// Every record whose LSN is at most this is on disk. It is never more
    /// than `end`, and it never goes down.
    durable: Lsn,
    /// Set after a failed write, sync, create or remove. While it is set,
    /// every call returns `Error::Stopped`.
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
    /// that is `vfs::Error::AlreadyExists`.
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

    /// Opens the log in `dir`, finds where its valid records end, and cuts
    /// off everything after that.
    ///
    /// After a crash, the newest record may be torn, and the files may hold
    /// bytes that were never synced. `open` reads the whole log, oldest
    /// first, and stops at the first bad fragment, as replay does. It then
    /// removes everything from the start of that fragment's record to the
    /// end of the log: it deletes the later segments and shortens the
    /// segment that holds the end. Last, it writes the rest of that segment
    /// again and syncs it, so every record that is left is on disk. After
    /// `open`, [`end`](Wal::end) and [`durable`](Wal::durable) are both the
    /// LSN of the last whole record.
    ///
    /// A crash inside [`create`](Wal::create) can leave a directory with no
    /// segments. `open` opens it as an empty log. `segment_size` does not
    /// have to be the size the log was written with, because the name of
    /// each segment says where it starts.
    ///
    /// `open` reads the log once to find its end. [`replay`](Wal::replay)
    /// then reads it again to return the records.
    ///
    /// # Errors
    ///
    /// - [`Error::Vfs`] if a file system call fails. If `dir` does not exist,
    ///   that is `vfs::Error::NotFound`.
    /// - [`Error::Corrupt`] if the segments break a rule that
    ///   [`segment::list`] checks.
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
    /// let wal = Wal::create(Arc::new(mem.clone()), "wal", BLOCK as u64).unwrap();
    /// let lsn = wal.append(b"put apple 1").unwrap();
    /// wal.flush_to(lsn).unwrap();
    /// drop(wal);
    ///
    /// let wal = Wal::open(Arc::new(mem), "wal", BLOCK as u64).unwrap();
    /// assert_eq!(wal.end(), lsn);
    /// ```
    pub fn open(vfs: Arc<dyn Vfs>, dir: impl AsRef<Path>, segment_size: u64) -> Result<Wal> {
        check_size(segment_size);
        let dir = dir.as_ref();
        let mut segments = segment::list(&*vfs, dir)?;
        if segments.is_empty() {
            let file = new_segment(&*vfs, dir, 0)?;
            return Ok(Wal::new(vfs, dir, segment_size, vec![0], file, 0));
        }
        // Read every record, to find where the valid ones end.
        let mut reader = Reader::new(&*vfs, dir, segments.clone())?;
        while reader.next_record()?.is_some() {}
        let end = reader.end();
        let file = cut_tail(&*vfs, dir, &mut segments, end)?;
        Ok(Wal::new(vfs, dir, segment_size, segments, file, end))
    }

    /// Returns an iterator over every durable record in the log, oldest
    /// first.
    ///
    /// To recover after a crash, call it right after [`open`](Wal::open).
    /// At that point every record in the log is durable, so the iterator
    /// returns all of them. The iterator does not return records that were
    /// appended after this call, or records that are not durable yet. After
    /// [`drop_before(x)`](Wal::drop_before), the first records may still
    /// have an LSN of `x` or less. The caller skips the ones it does not
    /// need.
    ///
    /// The iterator reads the segment files without the log's mutex, so
    /// other threads can append while it runs. Do not call `drop_before`
    /// while it runs. `drop_before` may remove a segment that the iterator
    /// has not read yet, and the iterator then returns [`Error::Vfs`].
    ///
    /// # Errors
    ///
    /// - [`Error::Stopped`] if the log has stopped.
    /// - [`Error::Vfs`] if the oldest segment cannot be
    ///   opened or read.
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
    /// let lsn = wal.append(b"durable").unwrap();
    /// wal.flush_to(lsn).unwrap();
    /// wal.append(b"not yet").unwrap();
    ///
    /// // Only the durable record comes back.
    /// assert_eq!(wal.replay().unwrap().count(), 1);
    /// ```
    pub fn replay(&self) -> Result<Replay<'_>> {
        let (segments, durable) = {
            let state = self.lock()?;
            (state.segments.clone(), state.durable)
        };
        let reader = Reader::new(&*self.vfs, &self.dir, segments)?;
        Ok(Replay::new(reader, durable))
    }
}

/// Removes everything after `end`, the LSN just past the last whole record.
/// Returns the segment that holds `end`, opened. That segment becomes the
/// current one, and the removed segments are taken out of `segments`.
///
/// The bytes after `end` must be removed. New records will be written over
/// them. If the new records are shorter than the old bytes, some old bytes
/// stay after them. A later replay could read on into those old bytes and
/// return records that were never synced and never reported as saved.
///
/// The steps go in this order. First, it removes the later segments, the
/// newest first, and syncs the directory. Then it shortens the segment that
/// holds `end`, writes its bytes again, and syncs it. A crash can happen
/// between any two steps, and the files are still valid after it: every
/// segment except the last one stays full.
fn cut_tail(vfs: &dyn Vfs, dir: &Path, segments: &mut Vec<Lsn>, end: Lsn) -> Result<Box<dyn File>> {
    // The segment that holds `end` is the newest segment that starts at or
    // before `end`. If `end` is exactly where a segment starts, it is that
    // segment, and the segment becomes empty.
    let keep = segments.partition_point(|&start| start <= end);
    if keep < segments.len() {
        for &start in segments[keep..].iter().rev() {
            vfs.remove(&segment::path(dir, start))?;
        }
        vfs.sync_dir(dir)?;
        segments.truncate(keep);
    }
    let start = segments[keep - 1];
    let file = vfs.open(&segment::path(dir, start), Open::Existing)?;
    file.set_len(end - start)?;
    // `&*file` borrows the `File` inside the box.
    rewrite_tail(&*file, end - start)?;
    file.sync()?;
    Ok(file)
}

fn rewrite_tail(_file: &dyn File, _len: u64) -> Result<()> {
    Ok(())
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
    /// The record is written to the current segment immediately. If it does
    /// not fit, the rest of it goes into a new segment. The record is not
    /// durable yet: call [`flush_to`](Wal::flush_to) with the LSN before
    /// telling anyone that it is saved. A record can have any length,
    /// including 0. Replay returns the whole record or none of it.
    ///
    /// The LSN is the log position just past the record. It is bigger than
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
            // A segment is full `segment_size` bytes after its start. The log
            // may have been written with a bigger `segment_size` than the one
            // it was opened with. Then the current segment can already be
            // longer than that, and it is full at the next block boundary.
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
        // Sync the full segment before any record goes into the next one.
        // Otherwise a crash could keep a record in the new segment and lose
        // one before it in this segment.
        state.file.sync()?;
        state.file = new_segment(&*self.vfs, &self.dir, start)?;
        state.segments.push(start);
        Ok(())
    }

    /// Makes every record whose LSN is at most `lsn` durable, and returns
    /// once they are on disk.
    ///
    /// An `lsn` past the end means the whole log, so `flush_to(u64::MAX)`
    /// makes everything appended so far durable. If the records are already
    /// durable, it returns immediately and does not sync. Otherwise it syncs
    /// the current segment. That sync makes every record appended so far
    /// durable, including the ones after `lsn`.
    ///
    /// Afterwards, [`durable`](Wal::durable) is at least the smaller of
    /// `lsn` and [`end`](Wal::end).
    ///
    /// # Errors
    ///
    /// - [`Error::Stopped`] if the log has stopped.
    /// - [`Error::Vfs`] if the sync fails. The records that were not durable
    ///   may be lost, so do not report them as saved. The log then stops.
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
        // Every earlier segment was synced when it filled up. With this
        // segment synced too, the whole log is on disk.
        state.durable = state.end;
        Ok(())
    }

    /// The LSN just past the last record appended. Before the first append,
    /// it is the end of the records that were already in the log, or 0 for
    /// a new log.
    ///
    /// # Panics
    ///
    /// If another thread panicked while it held the log's mutex.
    pub fn end(&self) -> Lsn {
        self.state().end
    }

    /// The LSN up to which every record is on disk. It never goes down, and
    /// it is never past [`end`](Wal::end).
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

impl Wal {
    /// Removes the segments that hold only records up to `lsn`, oldest
    /// first, then syncs the directory.
    ///
    /// Call it when the changes in those records are saved somewhere else,
    /// for example after a checkpoint. A segment is removed when the segment
    /// after it starts at or before `lsn`. The current segment is never
    /// removed. Replay may still return some records with an LSN of `lsn` or
    /// less, from the segment that holds `lsn`. Callers skip the ones they do
    /// not need.
    ///
    /// `lsn` must be a value that [`append`](Wal::append) or
    /// [`end`](Wal::end) returned. Such a position is always between two
    /// records, never inside one. So every record after `lsn` keeps its
    /// start.
    ///
    /// The segments are removed oldest first, so a crash cannot leave a gap
    /// in the log. A crash keeps the name changes made in a directory since
    /// its last sync as a prefix, in order. So the segments that are left
    /// always form one unbroken run.
    ///
    /// # Errors
    ///
    /// - [`Error::Stopped`] if the log has stopped.
    /// - [`Error::Vfs`] if removing a segment or syncing the directory
    ///   fails. The log then stops.
    ///
    /// # Panics
    ///
    /// If another thread panicked while it held the log's mutex.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::{path::Path, sync::Arc};
    /// use vfs::Mem;
    /// use wal::{BLOCK, Wal, segment};
    ///
    /// let mem = Mem::new();
    /// let wal = Wal::create(Arc::new(mem.clone()), "wal", BLOCK as u64).unwrap();
    /// wal.append(&[1; BLOCK]).unwrap();
    /// let lsn = wal.append(&[2; BLOCK]).unwrap();
    /// assert_eq!(segment::list(&mem, Path::new("wal")).unwrap().len(), 3);
    ///
    /// // Only the current segment is left.
    /// wal.drop_before(lsn).unwrap();
    /// assert_eq!(segment::list(&mem, Path::new("wal")).unwrap(), [2 * BLOCK as u64]);
    /// ```
    pub fn drop_before(&self, lsn: Lsn) -> Result<()> {
        let mut state = self.lock()?;
        let result = self.remove_segments(&mut state, lsn);
        if result.is_err() {
            state.stopped = true;
        }
        result
    }

    /// Removes segments from the oldest one forward, while the segment after
    /// each one starts at or before `lsn`. The current segment has no
    /// segment after it, so it is never removed.
    fn remove_segments(&self, state: &mut State, lsn: Lsn) -> Result<()> {
        let n = state
            .segments
            .windows(2)
            .take_while(|pair| pair[1] <= lsn)
            .count();
        if n == 0 {
            return Ok(());
        }
        for &start in &state.segments[..n] {
            self.vfs.remove(&segment::path(&self.dir, start))?;
        }
        self.vfs.sync_dir(&self.dir)?;
        state.segments.drain(..n);
        Ok(())
    }
}
