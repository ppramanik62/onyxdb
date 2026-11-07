//! Helpers the tests share: a log in memory, and records a test can
//! recognize.

// Each test file uses only some of these.
#![allow(dead_code)]

use std::sync::Arc;

use vfs::Mem;
use wal::{BLOCK, Wal};

/// One block per segment, so tests fill segments quickly.
pub const SEGMENT: u64 = BLOCK as u64;

/// A new log in `wal/` on `mem`.
pub fn create(mem: &Mem) -> Wal {
    Wal::create(Arc::new(mem.clone()), "wal", SEGMENT).unwrap()
}

/// Record number `i`: `i` as 8 bytes, then `len` bytes that count up from
/// `i`. Every record is different, so a test can tell them apart.
pub fn record(i: u64, len: usize) -> Vec<u8> {
    let mut r = i.to_le_bytes().to_vec();
    r.extend((0..len).map(|j| (i as usize + j) as u8));
    r
}

/// The number of a record that `record` made, or `None` if any byte is
/// different.
pub fn number(r: &[u8]) -> Option<u64> {
    let i = u64::from_le_bytes(r.get(..8)?.try_into().ok()?);
    (record(i, r.len() - 8) == r).then_some(i)
}
