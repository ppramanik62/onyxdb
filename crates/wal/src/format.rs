//! The bytes of the log: records cut into fragments, and fragments packed
//! into blocks.
//!
//! The log is one long stream of bytes, and a position in it is an
//! [`Lsn`]. The stream is cut into blocks of [`BLOCK`] bytes. Each record
//! is written as one or more fragments, and a fragment never crosses a block
//! boundary.
//!
//! ```text
//! a fragment
//! 0         4      6      7
//! ┌─────────┬──────┬──────┬─────────────────┐
//! │ CRC-32C │ len  │ kind │ data: len bytes │    numbers are little-endian
//! └─────────┴──────┴──────┴─────────────────┘
//!           └───── the CRC covers these ────┘
//!
//! a block of 32 KiB
//! ┌────────────┬──────────────┬───────────────────────┬─────────┐
//! │ fragment   │ fragment     │ fragment              │ trailer │
//! └────────────┴──────────────┴───────────────────────┴─────────┘
//! ```
//!
//! | Term | Meaning |
//! |---|---|
//! | block | 32 KiB of the log, starting at an LSN that's a multiple of `BLOCK`. |
//! | fragment | A 7-byte header and part of one record's bytes, inside one block. |
//! | kind | Which part of its record a fragment holds: full, first, middle or last. |
//! | trailer | The zeros at the end of a block when fewer than `HEADER` bytes are left in it. |
//!
//! A record that fits in what's left of its block is one `Full` fragment.
//! A longer one is a `First` fragment that fills the block, `Middle`
//! fragments that fill whole blocks, and a `Last` fragment.
//!
//! Blocks make a damaged log easy to read past: a reader can always find
//! the next fragment at the start of a block. They also cap a fragment's
//! length, so it fits in two bytes.
//!
//! When fewer than `HEADER` bytes are left in a block, no header fits. They
//! stay zero, and the next fragment starts in the next block. When exactly
//! `HEADER` bytes are left, a header with no data fits, so a record can
//! start with an empty `First` fragment. Treating that case like any other
//! keeps the writer simple.
//!
//! Bytes that were never written are zeros. A zeroed header has kind 0, which
//! no fragment has, so a reader that reaches the unwritten end of the log
//! stops there.
//!
//! The CRC covers the length and the kind as well as the data. If a crash
//! tears the length, the reader would take the wrong bytes as the data.
//! Those would almost never match the CRC anyway, but with the length
//! inside the CRC, a torn length fails the check by itself.

use crate::{Lsn, crc::crc32c};

/// The size of a block, in bytes. Every block starts at an LSN that's a
/// multiple of this.
///
/// 32 KiB is LevelDB's block size. A fragment's length must fit in the two
/// bytes of its header, so a block can be at most 64 KiB. A smaller block
/// would waste more of the log on headers and trailers.
pub const BLOCK: usize = 32 * 1024;

/// The size of a fragment's header: a 4-byte CRC, a 2-byte length and a
/// 1-byte kind.
pub const HEADER: usize = 7;

/// Which part of its record a fragment holds. The number is the byte stored
/// in the header.
///
/// # Examples
///
/// ```
/// use wal::format::{Kind, encode};
///
/// // A short record is one full fragment. The kind is byte 6 of the header.
/// let bytes = encode(0, b"hello");
/// assert_eq!(bytes[6], Kind::Full as u8);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A whole record.
    Full = 1,
    /// The start of a record that goes on in the next block.
    First = 2,
    /// A part of a record that started in an earlier block and goes on in
    /// the next.
    Middle = 3,
    /// The end of a record that started in an earlier block.
    Last = 4,
}

impl Kind {
    /// The kind stored as `byte`, or `None` if no kind has that number.
    fn from_byte(byte: u8) -> Option<Kind> {
        match byte {
            1 => Some(Kind::Full),
            2 => Some(Kind::First),
            3 => Some(Kind::Middle),
            4 => Some(Kind::Last),
            _ => None,
        }
    }
}

/// The bytes to write for a record whose first byte would go at log
/// position `at`.
///
/// The returned bytes belong at `at`. If fewer than [`HEADER`] bytes are
/// left in that block, they start with the zeros that finish it. Then come
/// the record's fragments, split at every block boundary. The record's LSN
/// is `at` plus the length of what's returned.
///
/// A record can be any length, including 0. It's copied once, into the
/// returned vector.
///
/// # Examples
///
/// A 10-byte record at the start of the log is one fragment of 17 bytes:
///
/// ```
/// use wal::format::{HEADER, Kind, encode};
///
/// let bytes = encode(0, b"0123456789");
/// assert_eq!(bytes.len(), HEADER + 10);
/// assert_eq!(bytes[6], Kind::Full as u8);
/// ```
///
/// Near the end of a block, the same record is split in two:
///
/// ```
/// use wal::format::{BLOCK, HEADER, Kind, encode};
///
/// let bytes = encode((BLOCK - 10) as u64, b"0123456789");
/// // 3 bytes fit in this block, and the other 7 go in the next one.
/// assert_eq!(bytes.len(), HEADER + 3 + HEADER + 7);
/// assert_eq!(bytes[6], Kind::First as u8);
/// assert_eq!(bytes[HEADER + 3 + 6], Kind::Last as u8);
/// ```
pub fn encode(at: Lsn, record: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER + record.len());
    let mut pos = at;
    let mut rest = record;
    let mut first = true;
    loop {
        let room = BLOCK - (pos % BLOCK as Lsn) as usize;
        if room < HEADER {
            // No header fits. Fill the rest of the block with zeros.
            out.resize(out.len() + room, 0);
            pos += room as Lsn;
            continue;
        }
        let n = rest.len().min(room - HEADER);
        let last = n == rest.len();
        let kind = match (first, last) {
            (true, true) => Kind::Full,
            (true, false) => Kind::First,
            (false, false) => Kind::Middle,
            (false, true) => Kind::Last,
        };
        let start = out.len();
        // Room for the CRC, which is filled in once the rest is written.
        out.extend_from_slice(&[0; 4]);
        // `n` is less than `BLOCK`, so it fits in a u16.
        out.extend_from_slice(&(n as u16).to_le_bytes());
        out.push(kind as u8);
        out.extend_from_slice(&rest[..n]);
        let crc = crc32c(&out[start + 4..]);
        out[start..start + 4].copy_from_slice(&crc.to_le_bytes());
        pos += (HEADER + n) as u64;
        rest = &rest[n..];
        first = false;
        if last {
            return out;
        }
    }
}

/// A fragment read back from a block.
///
/// # Examples
///
/// ```
/// use wal::format::{Fragment, Kind, decode, encode};
///
/// let bytes = encode(0, b"hi");
/// let expected = Fragment { kind: Kind::Full, data: b"hi" };
/// assert_eq!(decode(&bytes, 0), Some(expected));
/// ```
#[derive(Debug, PartialEq, Eq)]
pub struct Fragment<'a> {
    /// Which part of its record this is.
    pub kind: Kind,
    /// This fragment's part of the record's bytes.
    pub data: &'a [u8],
}

/// Reads the fragment that starts at byte `at` of `block`, if a valid one is
/// there.
///
/// `block` holds a block's bytes from its start. It's shorter than
/// [`BLOCK`] when the log's files end inside the block. Returns `None` when
/// the header doesn't fit in `block`, the kind isn't one of the four, the
/// data runs past the end of `block`, or the CRC doesn't match. A reader
/// treats `None` as the end of the log.
///
/// # Examples
///
/// ```
/// use wal::format::{Kind, decode, encode};
///
/// let bytes = encode(0, b"hello");
/// let fragment = decode(&bytes, 0).unwrap();
/// assert_eq!(fragment.kind, Kind::Full);
/// assert_eq!(fragment.data, b"hello");
///
/// // Zeros, like the unwritten end of the log, are not a fragment.
/// assert_eq!(decode(&[0; 32], 0), None);
/// ```
pub fn decode(block: &[u8], at: usize) -> Option<Fragment<'_>> {
    let header = block.get(at..at + HEADER)?;
    let len = u16::from_le_bytes([header[4], header[5]]) as usize;
    let kind = Kind::from_byte(header[6])?;
    let stored = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
    // The length, the kind and the data: everything the CRC covers
    let covered = block.get(at + 4..at + HEADER + len)?;
    if crc32c(covered) != stored {
        return None;
    }
    Some(Fragment {
        kind,
        data: &covered[HEADER - 4..],
    })
}
