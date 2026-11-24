//! LRU-K against plain LRU: one scan through many pages.

mod common;

use bufpool::{BufPool, LruK, NoLog};
use common::{file_with, no};
use vfs::Mem;

/// Fetches hot pages 1 to 5 three times each, then scans pages 100 to 199
/// once each, then fetches the hot pages again. Returns how many of those
/// last five fetches were hits.
fn hot_pages_after_a_scan(policy: impl FnOnce(usize) -> LruK) -> u64 {
    let mem = Mem::new();
    let pool = BufPool::new(file_with(&mem, 200), 10, policy, Box::new(NoLog));
    for _ in 0..3 {
        for n in 1..=5 {
            pool.fetch(no(n)).unwrap();
        }
    }
    for n in 100..200 {
        pool.fetch(no(n)).unwrap();
    }
    let before = pool.stats().hits;
    for n in 1..=5 {
        pool.fetch(no(n)).unwrap();
    }
    pool.stats().hits - before
}

#[test]
fn a_scan_flushes_lru_but_not_lru_2() {
    assert_eq!(hot_pages_after_a_scan(|frames| LruK::new(frames, 1)), 0);
    assert_eq!(hot_pages_after_a_scan(|frames| LruK::new(frames, 2)), 5);
}
