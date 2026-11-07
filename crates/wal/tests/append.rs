//! Appending, syncing, filling segments, and stopping after a failure.

mod common;

use std::path::Path;

use vfs::{Fault, Mem, Vfs};
use wal::{BLOCK, Error, HEADER};

#[test]
fn lsns_are_positions() {
    let mem = Mem::new();
    let wal = common::create(&mem);
    let first = wal.append(b"0123456789").unwrap();
    let second = wal.append(b"").unwrap();
    assert_eq!((first, second), (17, 24));
    assert_eq!((wal.end(), wal.durable()), (24, 0));

    // One sync covers both records, though we asked only for the first.
    wal.flush_to(first).unwrap();
    assert_eq!(wal.durable(), 24);
    // Past the end means everything appended so far.
    let third = wal.append(b"x").unwrap();
    wal.flush_to(u64::MAX).unwrap();
    assert_eq!(wal.durable(), third);
}

#[test]
fn segments_fill_up() {
    let mem = Mem::new();
    let wal = common::create(&mem);
    // Three records of half a block fill one-block segments.
    for i in 0..3 {
        wal.append(&common::record(i, BLOCK / 2)).unwrap();
    }
    let names: Vec<_> = mem.list(Path::new("wal")).unwrap();
    assert_eq!(
        names,
        [
            Path::new("wal/0000000000000000.wal"),
            Path::new("wal/0000000000008000.wal"),
        ]
    );
    // The full segment holds exactly one block.
    let file = mem.open(&names[0], vfs::Open::Existing).unwrap();
    assert_eq!(file.len().unwrap(), BLOCK as u64);
    assert_eq!(wal.end(), 3 * (BLOCK / 2 + 8) as u64 + 4 * HEADER as u64);
}

#[test]
fn create_refuses_an_existing_log() {
    let mem = Mem::new();
    common::create(&mem);
    let again = wal::Wal::create(std::sync::Arc::new(mem), "wal", common::SEGMENT);
    assert!(matches!(
        again,
        Err(Error::Vfs(vfs::Error::AlreadyExists(_)))
    ));
}

#[test]
fn a_failed_sync_stops_the_log() {
    let mem = Mem::new();
    let wal = common::create(&mem);
    let lsn = wal.append(b"lost").unwrap();
    mem.fail_next(Fault::SyncError);
    assert!(matches!(wal.flush_to(lsn), Err(Error::Vfs(_))));
    // Retrying would succeed on Linux, and say a lost record is safe.
    assert!(matches!(wal.flush_to(lsn), Err(Error::Stopped)));
    assert!(matches!(wal.append(b"more"), Err(Error::Stopped)));
    assert_eq!(wal.durable(), 0);
}

#[test]
fn a_failed_write_stops_the_log() {
    let mem = Mem::new();
    let wal = common::create(&mem);
    mem.fail_next(Fault::ShortWrite);
    assert!(matches!(wal.append(b"half of me"), Err(Error::Vfs(_))));
    assert!(matches!(wal.append(b"next"), Err(Error::Stopped)));
}
