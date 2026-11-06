//! Many threads on one pool.

mod common;

use std::path::Path;
use std::sync::Barrier;
use std::thread;
use std::time::Duration;

use bufpool::{BufPool, Clock, Error, LogHook};
use common::{STAMP, file_with, no, pool};
use pagefile::{Page, PageFile, PageStore};
use vfs::{Mem, Open, Rng, Vfs};

/// Each test page holds a counter in 60 words, starting at byte 16.
const WORDS: usize = 60;

/// The counter in `page`. Panics if its words disagree, which would mean two
/// threads changed the page at once, or a frame held a mix of two pages.
fn counter(page: &Page) -> u64 {
    let first = page.u64_at(16);
    for i in 1..WORDS {
        assert_eq!(page.u64_at(16 + 8 * i), first, "a torn page");
    }
    first
}

#[test]
fn eight_threads_agree_with_a_model() {
    const PAGES: u32 = 64;
    let mem = Mem::new();
    let pool = pool(&mem, PAGES, 12);
    // Each thread counts its own adds to each page: that's the model.
    let adds: Vec<Vec<u64>> = thread::scope(|s| {
        let threads: Vec<_> = (0..8)
            .map(|t| {
                let pool = &pool;
                s.spawn(move || {
                    let mut rng = Rng::new(t + 1);
                    let mut adds = vec![0; PAGES as usize + 1];
                    for _ in 0..2000 {
                        let n = 1 + rng.below(PAGES as usize) as u32;
                        if rng.below(4) < 3 {
                            let mut page = pool.fetch_mut(no(n)).unwrap();
                            let c = counter(&page);
                            for i in 0..WORDS {
                                page.put_u64(16 + 8 * i, c + 1);
                            }
                            adds[n as usize] += 1;
                        } else {
                            let page = pool.fetch(no(n)).unwrap();
                            counter(&page);
                            assert_eq!(page.u32_at(STAMP), n);
                        }
                    }
                    adds
                })
            })
            .collect();
        threads.into_iter().map(|t| t.join().unwrap()).collect()
    });
    // Every counter equals the sum of the adds, in the pool and in the file.
    pool.flush().unwrap();
    let file = PageFile::open(&mem, "test.pf").unwrap();
    let mut on_disk = file.new_page();
    for n in 1..=PAGES {
        let want: u64 = adds.iter().map(|a| a[n as usize]).sum();
        assert_eq!(counter(&pool.fetch(no(n)).unwrap()), want, "page {n}");
        file.read(no(n), &mut on_disk).unwrap();
        assert_eq!(counter(&on_disk), want, "page {n} in the file");
    }
    println!("{:?}", pool.stats());
}

#[test]
fn threads_missing_one_page_share_one_read() {
    let mem = Mem::new();
    let pool = pool(&mem, 10, 4);
    let start = Barrier::new(8);
    thread::scope(|s| {
        for _ in 0..8 {
            s.spawn(|| {
                start.wait();
                assert_eq!(pool.fetch(no(7)).unwrap().u32_at(STAMP), 7);
            });
        }
    });
    let stats = pool.stats();
    assert_eq!((stats.misses, stats.hits), (1, 7));
}

#[test]
fn a_failed_read_fails_every_waiting_thread() {
    let mem = Mem::new();
    let pool = pool(&mem, 11, 10);
    let file = mem.open(Path::new("test.pf"), Open::Existing).unwrap();
    file.write_at(&[0xff], 7 * 512 + 100).unwrap();
    let start = Barrier::new(8);
    thread::scope(|s| {
        for _ in 0..8 {
            s.spawn(|| {
                start.wait();
                let result = pool.fetch(no(7)).map(|page| page.u32_at(STAMP));
                assert!(matches!(result, Err(Error::PageFile(_))), "{result:?}");
            });
        }
    });
    // Every frame is free again: the ten good pages can all be held at once.
    let held: Vec<_> = (1..=11)
        .filter(|&n| n != 7)
        .map(|n| pool.fetch(no(n)).unwrap())
        .collect();
    assert_eq!(held.len(), 10);
}

/// A log whose flush takes 200 ms, for pages that have an LSN.
struct SlowLog;

impl LogHook for SlowLog {
    fn flush_to(&self, lsn: u64) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if lsn > 0 {
            thread::sleep(Duration::from_millis(200));
        }
        Ok(())
    }
}

#[test]
fn a_victim_being_written_can_still_be_fetched() {
    let mem = Mem::new();
    let pool = BufPool::new(
        file_with(&mem, 10),
        2,
        Box::new(Clock::new(2)),
        Box::new(SlowLog),
    );
    {
        let mut one = pool.fetch_mut(no(1)).unwrap();
        one.put_u32(STAMP, 100);
        one.set_lsn(7);
    }
    pool.fetch(no(2)).unwrap();
    thread::scope(|s| {
        // Page 1 is the victim. Its write-back waits 200 ms for the log.
        s.spawn(|| assert_eq!(pool.fetch(no(3)).unwrap().u32_at(STAMP), 3));
        thread::sleep(Duration::from_millis(50));
        // Meanwhile page 1 is still in the table, with its new bytes.
        assert_eq!(pool.fetch(no(1)).unwrap().u32_at(STAMP), 100);
    });
    assert_eq!(pool.fetch(no(1)).unwrap().u32_at(STAMP), 100);
}
