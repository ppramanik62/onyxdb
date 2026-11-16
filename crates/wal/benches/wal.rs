//! How fast the log appends and commits, on the real disk.
//!
//! Prints records per second for appends that are not synced, and commits
//! per second (an append, then `flush_to`) from 1 and 8 threads. Run it with
//! `cargo bench`, and compare with `benches/baseline.txt`.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use vfs::{Disk, Vfs};
use wal::{BLOCK, Wal};

/// Each measurement runs for about this long.
const RUN: Duration = Duration::from_secs(1);

/// A segment of 4 MiB.
const SEGMENT: u64 = 128 * BLOCK as u64;

fn main() {
    // Cargo's scratch directory for benchmarks, inside `target/`. Not
    // `/tmp`, which is often kept in memory, where a sync costs nothing.
    let disk = Arc::new(Disk::new(env!("CARGO_TARGET_TMPDIR")));
    let dir = PathBuf::from(format!("wal-bench-{}", std::process::id()));
    disk.create_dir(&dir).unwrap();
    let record = [7u8; 100];

    let wal = Wal::create(disk.clone(), dir.join("append"), SEGMENT).unwrap();
    let rate = per_second(1, || {
        wal.append(&record).unwrap();
    });
    println!("append, not synced   1 thread   {rate:>9} records/s");
    remove(&*disk, &dir.join("append"));

    for threads in [1, 8] {
        let log = dir.join(format!("commit-{threads}"));
        let wal = Wal::create(disk.clone(), &log, SEGMENT).unwrap();
        let rate = per_second(threads, || {
            let lsn = wal.append(&record).unwrap();
            wal.flush_to(lsn).unwrap();
        });
        let label = if threads == 1 { "thread " } else { "threads" };
        println!("commit               {threads} {label}  {rate:>9} commits/s");
        remove(&*disk, &log);
    }
}

/// Removes every file in `dir`, so runs do not fill the disk. The empty
/// directories stay until `cargo clean`.
fn remove(vfs: &dyn Vfs, dir: &Path) {
    for file in vfs.list(dir).unwrap() {
        vfs.remove(&file).unwrap();
    }
}

/// Runs `op` on `threads` threads for `RUN`, and returns how many times it
/// ran per second, all threads together.
fn per_second(threads: usize, op: impl Fn() + Sync) -> u64 {
    let start = Instant::now();
    let total: u64 = thread::scope(|s| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| {
                    let mut n = 0;
                    while start.elapsed() < RUN {
                        op();
                        n += 1;
                    }
                    n
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).sum()
    });
    (total as f64 / start.elapsed().as_secs_f64()) as u64
}
