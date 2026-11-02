//! Changed pages: written back when they're evicted, and never before the log.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use bufpool::{BufPool, Clock, Error, LogHook};
use common::{STAMP, file_with, no, pool};
use pagefile::{PageFile, PageStore};
use vfs::{Mem, Rng};

#[test]
fn a_change_survives_eviction() {
    let mem = Mem::new();
    let pool = pool(&mem, 10, 2);
    pool.fetch_mut(no(1)).unwrap().put_u32(STAMP, 100);
    // Two more pages need both frames, so page 1 is written back and evicted.
    pool.fetch(no(2)).unwrap();
    pool.fetch(no(3)).unwrap();
    assert_eq!(pool.stats().writes, 1);
    // Fetching it again reads it from the file, checksum and all.
    assert_eq!(pool.fetch(no(1)).unwrap().u32_at(STAMP), 100);
}

#[test]
fn a_clean_page_is_never_written() {
    let mem = Mem::new();
    let pool = pool(&mem, 10, 2);
    for n in 1..=10 {
        pool.fetch(no(n)).unwrap();
    }
    assert_eq!(pool.stats().writes, 0);
}

/// A log that says it's durable up to some LSN, and fails every third
/// flush. `durable` is the highest LSN it reported durable.
struct FlakyLog {
    durable: Arc<AtomicU64>,
    calls: AtomicU64,
}

impl LogHook for FlakyLog {
    fn flush_to(&self, lsn: u64) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if self.calls.fetch_add(1, Ordering::SeqCst) % 3 == 2 {
            return Err("the log disk is full".into());
        }
        self.durable.fetch_max(lsn, Ordering::SeqCst);
        Ok(())
    }
}

#[test]
fn no_page_reaches_the_file_before_its_log() {
    for seed in 1..=10 {
        let mut rng = Rng::new(seed);
        let mem = Mem::new();
        let durable = Arc::new(AtomicU64::new(0));
        let log = FlakyLog {
            durable: durable.clone(),
            calls: AtomicU64::new(0),
        };
        let pool = BufPool::new(
            file_with(&mem, 30),
            4,
            Box::new(Clock::new(4)),
            Box::new(log),
        );
        let mut lsn = 0;
        for _ in 0..500 {
            let n = no(1 + rng.below(30) as u32);
            // A failed log flush stops the eviction it was for. That's fine here.
            let result = if rng.below(2) == 0 {
                lsn += 1;
                pool.fetch_mut(n).map(|mut page| page.set_lsn(lsn))
            } else {
                pool.fetch(n).map(drop)
            };
            assert!(matches!(result, Ok(()) | Err(Error::Log(_))));
        }
        // Every page in the file has an LSN the log said was durable.
        let file = PageFile::open(&mem, "test.pf").unwrap();
        let mut page = file.new_page();
        for n in 1..=30 {
            file.read(no(n), &mut page).unwrap();
            assert!(
                page.lsn() <= durable.load(Ordering::SeqCst),
                "seed {seed}, page {n}"
            );
        }
    }
}
