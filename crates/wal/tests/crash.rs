//! Crashes at every write, and failed syncs, through `vfs::crash_test`.

mod common;

use std::sync::Arc;

use vfs::{Fault, Mem, Rng, crash_test};
use wal::{Error, Lsn, Wal};

/// What the workload promised: the checks after a crash hold it to this.
#[derive(Default)]
struct Promised {
    /// The LSN of every record appended, by number.
    lsns: Vec<Lsn>,
    /// Records before this number were reported durable.
    acked: usize,
    /// Every record from this number on must survive the drops.
    kept_from: usize,
}

/// Turns an error from the log into the file system error the crash test
/// expects. Any other error is a bug.
fn vfs_error(e: Error) -> vfs::Error {
    match e {
        Error::Vfs(e) => e,
        other => panic!("{other}"),
    }
}

/// Appends 40 records of up to 12,000 bytes to a log of one-block segments.
/// It flushes after about a third of them, drops old segments every 10
/// records, and closes and opens the log once. If `fail` is
/// `Some(n)`, the `n`th flush fails its sync, and the workload opens the log
/// again and continues.
fn workload(mem: &Mem, promised: &mut Promised, fail: Option<usize>) -> vfs::Result<()> {
    let vfs = Arc::new(mem.clone());
    let mut wal = Wal::create(vfs.clone(), "wal", common::SEGMENT).map_err(vfs_error)?;
    let mut rng = Rng::new(7);
    let mut flushes = 0;
    for i in 0..40 {
        let record = common::record(i, rng.below(12_000));
        let lsn = wal.append(&record).map_err(vfs_error)?;
        promised.lsns.push(lsn);
        if rng.below(3) == 0 {
            if fail == Some(flushes) {
                mem.fail_next(Fault::SyncError);
            }
            flushes += 1;
            match wal.flush_to(lsn) {
                Ok(()) => promised.acked = i as usize + 1,
                Err(Error::Vfs(vfs::Error::Injected(_))) => {
                    // The records since the last flush may be lost. Not
                    // retrying: open the log again, which reads what is left.
                    wal = Wal::open(vfs.clone(), "wal", common::SEGMENT).map_err(vfs_error)?;
                }
                Err(e) => return Err(vfs_error(e)),
            }
        }
        if i % 10 == 9 && promised.acked > 0 {
            promised.kept_from = promised.acked;
            wal.drop_before(promised.lsns[promised.acked - 1])
                .map_err(vfs_error)?;
        }
        if i == 25 {
            drop(wal);
            wal = Wal::open(vfs.clone(), "wal", common::SEGMENT).map_err(vfs_error)?;
        }
    }
    Ok(())
}

/// Opens the log after a crash and checks what is left: a run of records in
/// order, with the LSNs they were given, holding every acknowledged record
/// that was not dropped.
fn check(mem: &Mem, promised: &Promised) -> Result<(), String> {
    let wal = match common::open(mem) {
        Ok(wal) => wal,
        // A crash inside `create` can leave no log at all. That is fine if
        // nothing was acknowledged yet.
        Err(Error::Vfs(vfs::Error::NotFound(_))) if promised.lsns.is_empty() => return Ok(()),
        Err(e) => return Err(e.to_string()),
    };
    let mut numbers = Vec::new();
    for record in wal.replay().map_err(|e| e.to_string())? {
        let record = record.map_err(|e| e.to_string())?;
        let i = common::number(&record.data).ok_or("a record we never wrote")?;
        if promised.lsns.get(i as usize) != Some(&record.lsn) {
            return Err(format!("record {i} has LSN {}", record.lsn));
        }
        numbers.push(i as usize);
    }
    let first = numbers.first().copied().unwrap_or(promised.lsns.len());
    if numbers != (first..first + numbers.len()).collect::<Vec<_>>() {
        return Err(format!("records out of order or missing: {numbers:?}"));
    }
    if promised.acked > promised.kept_from {
        let last = first + numbers.len();
        if first > promised.kept_from || last < promised.acked {
            return Err(format!(
                "records {}..{} were acknowledged, but replay has {first}..{last}",
                promised.kept_from, promised.acked
            ));
        }
    }
    Ok(())
}

#[test]
fn crash_at_every_write() {
    let report = crash_test(4, |mem, p| workload(mem, p, None), check);
    println!("crash at every write: {report}");
    assert!(report.failures.is_empty());
}

#[test]
fn fail_every_sync_once() {
    let (mut runs, mut failed) = (0, 0);
    // The workload flushes 14 times.
    for n in 0..14 {
        let report = crash_test(2, |mem, p| workload(mem, p, Some(n)), check);
        if let Some(first) = report.failures.first() {
            println!("sync {n} fails, then: {report}");
            println!(
                "  e.g. power off after {} changes: {}",
                first.changes, first.message
            );
        }
        runs += report.runs;
        failed += report.failures.len();
    }
    println!("a failed sync at each flush: {runs} crashes, {failed} failed");
    assert_eq!(failed, 0);
}
