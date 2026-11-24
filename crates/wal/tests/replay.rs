//! Reading records back, and cutting off a torn or damaged tail.

mod common;

use std::path::Path;

use vfs::{Mem, Open, Rng, Vfs};
use wal::{BLOCK, Error, Lsn, segment};

/// A log of four synced records over three segments, the third record
/// longer than two blocks. Returns the memory it is in and the records' LSNs.
fn four_records() -> (Mem, Vec<Lsn>) {
    log_of(&[100, 2000, 70_000, 300])
}

/// A log of synced records of these lengths.
fn log_of(lengths: &[usize]) -> (Mem, Vec<Lsn>) {
    let mem = Mem::new();
    let wal = common::create(&mem);
    let lsns: Vec<Lsn> = lengths
        .iter()
        .enumerate()
        .map(|(i, &len)| wal.append(&common::record(i as u64, len)).unwrap())
        .collect();
    wal.flush_to(u64::MAX).unwrap();
    (mem, lsns)
}

/// Writes `bytes` at log position `at`, in the segment that holds it, and
/// syncs, as damage that a crash or a bad disk could leave.
fn damage(mem: &Mem, at: Lsn, bytes: &[u8]) {
    let start = at / common::SEGMENT * common::SEGMENT;
    let file = mem
        .open(&segment::path(Path::new("wal"), start), Open::Existing)
        .unwrap();
    file.write_at(bytes, at - start).unwrap();
    file.sync().unwrap();
}

#[test]
fn records_come_back_after_open() {
    let (mem, lsns) = four_records();
    let wal = common::open(&mem).unwrap();
    assert_eq!(common::replayed(&wal), [0, 1, 2, 3]);
    assert_eq!(wal.end(), lsns[3]);
    let got: Vec<Lsn> = wal.replay().unwrap().map(|r| r.unwrap().lsn).collect();
    assert_eq!(got, lsns);
}

#[test]
fn only_durable_records_are_replayed() {
    let (mem, _) = four_records();
    let wal = common::open(&mem).unwrap();
    wal.append(&common::record(4, 10)).unwrap();
    assert_eq!(common::replayed(&wal), [0, 1, 2, 3]);
}

#[test]
fn an_empty_directory_is_an_empty_log() {
    // What a crash right after `create` made the directory leaves.
    let mem = Mem::new();
    mem.create_dir(Path::new("wal")).unwrap();
    let wal = common::open(&mem).unwrap();
    assert_eq!(wal.end(), 0);
    assert_eq!(common::replayed(&wal), [] as [u64; 0]);
}

#[test]
fn a_torn_last_record_is_cut_off() {
    let (mem, lsns) = four_records();
    // Zero one byte of the last record's data: its CRC no longer matches.
    damage(&mem, lsns[3] - 1, &[0]);
    let wal = common::open(&mem).unwrap();
    assert_eq!(common::replayed(&wal), [0, 1, 2]);
    assert_eq!(wal.end(), lsns[2]);

    // New records go where the torn one was, and come back after it.
    let lsn = wal.append(&common::record(9, 50)).unwrap();
    wal.flush_to(lsn).unwrap();
    drop(wal);
    assert_eq!(common::replayed(&common::open(&mem).unwrap()), [0, 1, 2, 9]);
}

#[test]
fn records_after_a_tear_are_cut_off_too() {
    // Four records, all in the first segment.
    let (mem, lsns) = log_of(&[100, 2000, 300, 300]);
    // Zero the second record's header. Records 2 and 3 are intact, but the
    // log ends at the tear.
    damage(&mem, lsns[0], &[0; 7]);
    let wal = common::open(&mem).unwrap();
    assert_eq!(common::replayed(&wal), [0]);

    // A new record as long as record 1 ends exactly where record 2 starts.
    // Replay must still not reach record 2.
    let lsn = wal.append(&common::record(9, 2000)).unwrap();
    wal.flush_to(lsn).unwrap();
    drop(wal);
    assert_eq!(common::replayed(&common::open(&mem).unwrap()), [0, 9]);
}

#[test]
fn later_segments_are_removed() {
    let (mem, lsns) = four_records();
    assert_eq!(segment::list(&mem, Path::new("wal")).unwrap().len(), 3);
    damage(&mem, lsns[1] - 1, &[0]);
    let wal = common::open(&mem).unwrap();
    assert_eq!(common::replayed(&wal), [0]);
    assert_eq!(segment::list(&mem, Path::new("wal")).unwrap(), [0]);
}

#[test]
fn files_that_break_the_rules_are_corrupt() {
    let (mem, _) = four_records();
    mem.open(Path::new("wal/notes.txt"), Open::CreateNew)
        .unwrap();
    assert!(matches!(common::open(&mem), Err(Error::Corrupt { .. })));

    let (mem, _) = four_records();
    // Make a segment that is not the last one too short.
    let first = mem
        .open(&segment::path(Path::new("wal"), 0), Open::Existing)
        .unwrap();
    first.set_len(100).unwrap();
    let Err(e) = common::open(&mem) else { panic!() };
    assert_eq!(
        e.to_string(),
        "wal/0000000000000000.wal: the segment holds 100 bytes, but the next one starts 32768 bytes after it"
    );
}

#[test]
fn damaged_logs_never_panic() {
    for seed in vfs::seeds(200) {
        let (mem, _) = four_records();
        let mut rng = Rng::new(seed);
        for _ in 0..1 + rng.below(4) {
            let at = rng.below(3 * BLOCK) as Lsn;
            let len = 1 + rng.below(20);
            damage(&mem, at, &rng.bytes(len));
        }
        // Whatever is left must be the first few records, in order.
        let wal = common::open(&mem).unwrap();
        let got = common::replayed(&wal);
        assert_eq!(
            got,
            (0..got.len() as u64).collect::<Vec<_>>(),
            "seed {seed}"
        );
    }
}
