#![doc = include_str!("../README.md")]
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
