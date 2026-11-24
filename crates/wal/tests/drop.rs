//! Dropping old segments, and replay starting in the middle of a record.

mod common;

use std::path::Path;

use vfs::{Mem, Rng};
use wal::{BLOCK, Lsn, segment};

#[test]
fn whole_segments_are_dropped() {
    let mem = Mem::new();
    let wal = common::create(&mem);
    let lsns: Vec<Lsn> = (0..6)
        .map(|i| wal.append(&common::record(i, BLOCK / 3)).unwrap())
        .collect();
    wal.flush_to(u64::MAX).unwrap();
    let before = segment::list(&mem, Path::new("wal")).unwrap();
    assert_eq!(before, [0, 32768, 65536]);

    // Record 3 ends in the second segment, so only the first can go.
    wal.drop_before(lsns[3]).unwrap();
    assert_eq!(
        segment::list(&mem, Path::new("wal")).unwrap(),
        [32768, 65536]
    );
    // The current segment always stays.
    wal.drop_before(wal.end()).unwrap();
    assert_eq!(segment::list(&mem, Path::new("wal")).unwrap(), [65536]);
}

#[test]
fn replay_skips_the_rest_of_a_dropped_record() {
    let mem = Mem::new();
    let wal = common::create(&mem);
    // Record 1 starts in the first segment and ends in the second.
    let lsns: Vec<Lsn> = (0..4)
        .map(|i| wal.append(&common::record(i, 20_000)).unwrap())
        .collect();
    wal.flush_to(u64::MAX).unwrap();
    wal.drop_before(lsns[1]).unwrap();
    // The second segment starts with record 1's last fragment.
    assert_eq!(common::replayed(&wal), [2, 3]);
    drop(wal);
    assert_eq!(common::replayed(&common::open(&mem).unwrap()), [2, 3]);
}

#[test]
fn random_work_matches_a_model() {
    for seed in vfs::seeds(50) {
        let mem = Mem::new();
        let mut wal = common::create(&mem);
        let mut rng = Rng::new(seed);
        // The model: every record appended, by number, with its LSN.
        let mut model: Vec<Lsn> = Vec::new();
        // Every record from this number on must survive the drops.
        let mut kept_from = 0;
        for _ in 0..60 {
            match rng.below(10) {
                0..=5 => {
                    let i = model.len() as u64;
                    model.push(
                        wal.append(&common::record(i, rng.below(100 * 1024)))
                            .unwrap(),
                    );
                }
                6 | 7 => wal.flush_to(u64::MAX).unwrap(),
                8 if !model.is_empty() => {
                    let last = rng.below(model.len());
                    wal.flush_to(u64::MAX).unwrap();
                    wal.drop_before(model[last]).unwrap();
                    kept_from = kept_from.max(last + 1);
                }
                _ => {
                    // Without a crash, every record appended is still there.
                    drop(wal);
                    wal = common::open(&mem).unwrap();
                }
            }
        }
        wal.flush_to(u64::MAX).unwrap();
        let got: Vec<(u64, Lsn)> = wal
            .replay()
            .unwrap()
            .map(|r| {
                let r = r.unwrap();
                (common::number(&r.data).unwrap(), r.lsn)
            })
            .collect();
        // Replay returns the model's records from some point on, with their
        // LSNs, and every record after the last one dropped.
        let first = got.first().map_or(model.len(), |&(i, _)| i as usize);
        let expected: Vec<(u64, Lsn)> =
            (first..model.len()).map(|i| (i as u64, model[i])).collect();
        assert_eq!(got, expected, "seed {seed}");
        assert!(
            first <= kept_from,
            "seed {seed}: record {kept_from} is missing"
        );
    }
}
