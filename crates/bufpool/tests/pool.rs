//! Fetching pages: hits, misses, eviction and pins.

mod common;

use std::path::Path;

use bufpool::Error;
use common::{STAMP, no, pool};
use vfs::{Mem, Open, Vfs};

#[test]
fn a_second_fetch_is_a_hit() {
    let mem = Mem::new();
    let pool = pool(&mem, 10, 3);
    for n in [1, 2, 1, 1, 3] {
        let page = pool.fetch(no(n)).unwrap();
        assert_eq!(page.u32_at(STAMP), n);
    }
    let stats = pool.stats();
    assert_eq!((stats.hits, stats.misses, stats.evictions), (2, 3, 0));
}

#[test]
fn a_full_pool_evicts() {
    let mem = Mem::new();
    let pool = pool(&mem, 10, 3);
    for n in 1..=10 {
        assert_eq!(pool.fetch(no(n)).unwrap().u32_at(STAMP), n);
    }
    let stats = pool.stats();
    assert_eq!((stats.misses, stats.evictions), (10, 7));
}

#[test]
fn pinned_pages_stay() {
    let mem = Mem::new();
    let pool = pool(&mem, 10, 3);
    let one = pool.fetch(no(1)).unwrap();
    let two = pool.fetch(no(2)).unwrap();
    let three = pool.fetch(no(3)).unwrap();
    assert!(matches!(
        pool.fetch(no(4)),
        Err(Error::AllPinned { frames: 3 })
    ));
    // Unpin page 2. It's the only page that can go, so page 4 takes its frame.
    drop(two);
    let four = pool.fetch(no(4)).unwrap();
    assert_eq!(four.u32_at(STAMP), 4);
    assert_eq!(one.u32_at(STAMP), 1);
    assert_eq!(three.u32_at(STAMP), 3);
}

#[test]
fn a_damaged_page_is_caught_on_load() {
    let mem = Mem::new();
    let pool = pool(&mem, 3, 1);
    // Change one byte of page 2 in the file, behind the pool's back.
    let file = mem.open(Path::new("test.pf"), Open::Existing).unwrap();
    file.write_at(&[0xff], 2 * 512 + 100).unwrap();
    assert!(matches!(
        pool.fetch(no(2)),
        Err(Error::PageFile(pagefile::Error::Checksum { page: 2, .. }))
    ));
    // The frame went back to the empty list, so this one-frame pool still works.
    assert_eq!(pool.fetch(no(3)).unwrap().u32_at(STAMP), 3);
}
