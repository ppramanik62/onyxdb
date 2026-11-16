//! A write-ahead log: an append-only file of records that survives a crash.
//!
//! A database changes its data in memory and writes the changed pages to its
//! data files later. Before it tells anyone that a change is saved, it
//! appends a record of the change to the log and syncs the log. After a
//! crash, it reads the log back and applies the records again, so no saved
//! change is lost.
//!
//! The log is synced on every commit, and the data files are not. One change
//! can touch many pages in different places in the data files, and writing
//! all of them is slow. The record of the change is written in one place, at
//! the end of one file, which is the fastest write a disk can do.
//!
//! | Term | Meaning |
//! |---|---|
//! | record | The bytes a caller appends. The log does not look inside them. |
//! | LSN | Log sequence number: a position in the log, in bytes. A record's LSN is the position just past it. |
//! | durable | On disk, so a crash cannot take it away. |
//! | segment | One file of the log. The log is stored in a run of segments. |
//! | replay | Reading every durable record back, oldest first, after a restart. |
//!
//! ```text
//!  append(record) ──▶ LSN            written to the file, not durable yet
//!  flush_to(LSN)                     synced: the record is durable
//!  ... crash ...
//!  open, then replay ──▶ records     every durable record, oldest first
//! ```
//!
//! The log stores records and nothing else. The code that appends the
//! records decides what they mean, which ones belong together, and what to do
//! with them after a crash. The log promises one thing about each record:
//! after a crash, replay returns the whole record or none of it, whatever its
//! length.
//!
//! - `crc`: CRC-32C, the checksum that catches a torn or damaged fragment.
//! - `format`: how a record is cut into fragments and packed into blocks.
//! - `segment`: the segment files and their names.
//! - `wal`: `Wal`, which appends records and makes them durable.
//! - `replay`: reading records back, and finding where the valid log ends.

// Every public item must say what it is. The compiler checks this for us.
#![warn(missing_docs)]

/// A position in the log, in bytes from the start of the very first
/// segment.
///
/// A record's LSN is the position just past its last byte. PostgreSQL uses
/// the same position for the LSN it stores in a page. Every record has at
/// least a header, so its last byte is at position 0 or later, and its LSN
/// is at least 1. That leaves 0 free to mean "never logged". A record
/// appended later always has a bigger LSN.
pub type Lsn = u64;

pub mod crc;
mod error;
pub mod format;
mod replay;
pub mod segment;
pub mod wal;

pub use error::{Error, Result};
pub use format::{BLOCK, HEADER};
pub use replay::{Record, Replay};
pub use wal::Wal;
