//! Reading the log back: fragments across segments, records from fragments,
//! and [`Wal::replay`].
//!
//! The reader walks the log one block at a time, oldest segment first. In
//! each block it reads fragments with [`format::decode`] and puts records
//! back together:
//!
//! ```text
//!  fragment   outside a record           inside a record
//!  full       a whole record             out of order: the log ends
//!  first      starts a record            out of order: the log ends
//!  middle     skipped at the log's       adds to the record
//!             start, else the log ends
//!  last       the same as middle         ends it: a whole record
//! ```
//!
//! The reader stops at the first bad fragment. A fragment is bad when its
//! header is all zeros, its kind is unknown, its length runs past the end of
//! the block or the file, its CRC does not match, or it comes out of order.
//! The record that the bad fragment belongs to is not complete, so the
//! reader drops it. The log's valid end is the position just past the last
//! whole record.
//!
//! The reader stops there even when the bytes after it look valid. This can
//! happen after a crash. A disk saves data one sector at a time, and a crash
//! can keep a later sector of data that was not synced yet and lose an
//! earlier one. The log then has a hole, and after the hole there can be
//! fragments with correct CRCs.
//!
//! The reader does not need those fragments. A sync makes every earlier
//! write to the file durable. If the bytes after the hole had been synced,
//! the bytes in the hole would have been synced too, and there would be no
//! hole. So the bytes after a hole were never synced, and no caller was told
//! that their records were saved. Dropping them breaks no promise.
//!
//! The reader must not skip the hole and continue either. A record after the
//! hole would come back while a record before it is missing, and the log
//! promises that records come back in the order they were appended.
//!
//! This rule has a cost. A flipped bit in data that was synced also makes a
//! bad fragment. The reader stops there too, so every record after it is
//! lost, and no error says so. To tell the two cases apart, the log would
//! have to store how far it was synced, in a place that is synced too.
//!
//! The first segment can start in the middle of a record. This happens when
//! `Wal::drop_before` removed the segment that held the start of the record.
//! The reader cannot use the end of a record without its start, so it skips
//! `middle` and `last` fragments until the first record starts.

use std::{ops::Range, path::Path};

use vfs::{File, Open, Vfs};

use crate::{
    BLOCK, HEADER, Lsn, Result,
    format::{self, Kind},
    segment,
};

/// Reads the log's records in order, from the start of its oldest segment
/// to its valid end.
pub(crate) struct Reader<'a> {
    /// The file system the segments are read from.
    vfs: &'a dyn Vfs,
    /// The directory that holds the segments.
    dir: &'a Path,
    /// The start of every segment, oldest first.
    segments: Vec<Lsn>,
    /// The index in `segments` of the segment that `file` has open.
    index: usize,
    /// The segment the current block is in.
    file: Box<dyn File>,
    /// That segment's length, read when it was opened.
    len: u64,
    /// The current block's bytes. Shorter than `BLOCK` only where the log's
    /// files end inside the block.
    block: Vec<u8>,
    /// The LSN of `block[0]`.
    block_start: Lsn,
    /// Where the next fragment starts in `block`.
    at: usize,
    /// Whether a record has started yet. Until one has, fragments that
    /// continue a record are skipped.
    started: bool,
    /// The LSN just past the last whole record read, or the start of the
    /// oldest segment if none has been.
    end: Lsn,
}

impl<'a> Reader<'a> {
    /// A reader at the start of the first of `segments`, with its first
    /// block read.
    ///
    /// # Errors
    ///
    /// `Error::Vfs` if the first segment cannot be opened or read.
    pub(crate) fn new(vfs: &'a dyn Vfs, dir: &'a Path, segments: Vec<Lsn>) -> Result<Reader<'a>> {
        let file = vfs.open(&segment::path(dir, segments[0]), Open::Existing)?;
        let len = file.len()?;
        let mut reader = Reader {
            vfs,
            dir,
            block_start: segments[0],
            end: segments[0],
            segments,
            index: 0,
            file,
            len,
            block: Vec::with_capacity(BLOCK),
            at: 0,
            started: false,
        };
        reader.read_block()?;
        Ok(reader)
    }

    /// The LSN just past the last whole record read so far.
    pub(crate) fn end(&self) -> Lsn {
        self.end
    }

    /// Reads the block that starts at `block_start` into `block`, moving on
    /// to the next segment when this one ends there. Returns `false` when no
    /// segment holds that block, which means the reader has reached the end
    /// of the log's files.
    fn read_block(&mut self) -> Result<bool> {
        let mut offset = self.block_start - self.segments[self.index];
        if offset >= self.len {
            let next = self.index + 1;
            if next == self.segments.len() || self.segments[next] != self.block_start {
                return Ok(false);
            }
            let path = segment::path(self.dir, self.block_start);
            self.file = self.vfs.open(&path, Open::Existing)?;
            self.len = self.file.len()?;
            self.index = next;
            offset = 0;
        }
        let n = (self.len - offset).min(BLOCK as u64) as usize;
        self.block.resize(n, 0);
        self.file.read_at(&mut self.block, offset)?;
        self.at = 0;
        Ok(n > 0)
    }

    /// The next fragment's kind and where its data is in `block`, or `None`
    /// at the end of the log.
    fn fragment(&mut self) -> Result<Option<(Kind, Range<usize>)>> {
        if BLOCK - self.at < HEADER {
            // No header fits in the rest of this block. It is the trailer,
            // so the next fragment is at the start of the next block.
            self.block_start += BLOCK as u64;
            if !self.read_block()? {
                return Ok(None);
            }
        }
        let (kind, len) = match format::decode(&self.block, self.at) {
            Some(fragment) => (fragment.kind, fragment.data.len()),
            None => return Ok(None),
        };
        let data = self.at + HEADER..self.at + HEADER + len;
        self.at = data.end;
        Ok(Some((kind, data)))
    }
    /// The next whole record, or `None` at the log's valid end.
    ///
    /// # Errors
    ///
    /// `Error::Vfs` if a segment cannot be opened or read.
    pub(crate) fn next_record(&mut self) -> Result<Option<Record>> {
        let mut data = Vec::new();
        let mut inside = false;
        loop {
            let Some((kind, range)) = self.fragment()? else {
                return Ok(None);
            };
            let bytes = &self.block[range];
            match (inside, kind) {
                (false, Kind::Full) | (true, Kind::Last) => {
                    data.extend_from_slice(bytes);
                    break;
                }
                (false, Kind::First) => {
                    data.extend_from_slice(bytes);
                    inside = true;
                }
                (true, Kind::Middle) => data.extend_from_slice(bytes),
                // The rest of a record whose beginning was in a removed
                // segment.
                (false, Kind::Middle | Kind::Last) if !self.started => {}
                // A fragment out of order: the record before it was torn.
                _ => return Ok(None),
            }
        }
        self.started = true;
        self.end = self.block_start + self.at as u64;
        Ok(Some(Record {
            lsn: self.end,
            data,
        }))
    }
}

/// A record read back from the log.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use vfs::Mem;
/// use wal::{BLOCK, Record, Wal};
///
/// let wal = Wal::create(Arc::new(Mem::new()), "wal", BLOCK as u64).unwrap();
/// let lsn = wal.append(b"put apple 1").unwrap();
/// wal.flush_to(lsn).unwrap();
///
/// let first = wal.replay().unwrap().next().unwrap().unwrap();
/// assert_eq!(first, Record { lsn, data: b"put apple 1".to_vec() });
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// The record's LSN: the log position just past it, as
    /// [`Wal::append`](crate::Wal::append) returned it.
    pub lsn: Lsn,
    /// The bytes that were appended.
    pub data: Vec<u8>,
}

/// An iterator over the log's durable records, oldest first. Made by
/// [`Wal::replay`](crate::Wal::replay).
///
/// Each item is a `Result`, because reading a segment can fail. After an
/// error, the iterator returns `None`.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use vfs::Mem;
/// use wal::{BLOCK, Wal};
///
/// let wal = Wal::create(Arc::new(Mem::new()), "wal", BLOCK as u64).unwrap();
/// wal.append(b"a").unwrap();
/// wal.append(b"b").unwrap();
/// wal.flush_to(u64::MAX).unwrap();
///
/// let records: Vec<Vec<u8>> = wal.replay().unwrap().map(|r| r.unwrap().data).collect();
/// assert_eq!(records, [b"a", b"b"]);
/// ```
pub struct Replay<'a> {
    /// Reads the records from the segment files.
    reader: Reader<'a>,
    /// Records with an LSN past this one may not be on disk, so the
    /// iterator does not return them.
    durable: Lsn,
    /// Set at the end or after an error, so `next` returns `None` from then
    /// on.
    done: bool,
}

impl Iterator for Replay<'_> {
    type Item = Result<Record>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        match self.reader.next_record() {
            Ok(Some(record)) if record.lsn <= self.durable => Some(Ok(record)),
            Ok(_) => {
                self.done = true;
                None
            }
            Err(e) => {
                self.done = true;
                Some(Err(e))
            }
        }
    }
}

impl<'a> Replay<'a> {
    /// An iterator that returns the records `reader` reads, up to and
    /// including the one whose LSN is `durable`.
    pub(crate) fn new(reader: Reader<'a>, durable: Lsn) -> Replay<'a> {
        Replay {
            reader,
            durable,
            done: false,
        }
    }
}
