//! A write-ahead log: an append-only file of records that survives a crash.
//!
//! A database changes its data in memory, and writes the changes to its
//! data files later. Before it tells anyone a change is saved, it appends a
//! record of the change to the log and syncs the log. After a crash, it reads
//! the log back and applies the records again. Appending to the end of one
//! file is the fastest way to get bytes onto a disk, which is why the log
//! comes first.
//!
//! | Term | Meaning |
//! |---|---|
//! | record | The bytes a caller appends. The log doesn't look inside them. |
//! | LSN | Log sequence number: a position in the log, in bytes. A record's LSN is the position just past it. |
//! | durable | On disk, so a crash can't take it away. |
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
//! The log stores records and nothing else. What a record means, which
//! records belong together, and what to do with them after a crash is up to
//! the code that appends them. A record of any length is all or nothing:
//! after a crash, replay returns it whole or not at all.
//!
//! - `crc`: CRC-32C, the checksum that catches a torn or damaged fragment.
//! - `format`: how a record is cut into fragments and packed into blocks.
//! - `segment`: the segment files and their names.
//! - `wal`: `Wal`, which appends records and makes them durable.
//! - `replay`: reading records back, and finding where the valid log ends.

// Every public item must say what it is. The compiler checks this for us.
#![warn(missing_docs)]

/// A position in the log, in bytes from the start of the very first segment.
///
/// A record's LSN is the position just past its last byte. PostgreSQL
/// defines a page's LSN the same way. So no record has LSN 0, and 0 can mean
/// "never logged". LSNs only grow: a record appended later always has a
/// bigger one.
pub type Lsn = u64;

pub mod crc;
pub mod format;

pub use format::{BLOCK, HEADER};
