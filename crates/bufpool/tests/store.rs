//! The pool as a `PageStore`: the freelist, overflow chains and `flush`.

mod common;

use std::collections::HashMap;
use std::thread;
use std::time::Duration;

use bufpool::{BufPool, Clock, Error, NoLog};
use common::{STAMP, no, pool};
use pagefile::check::check;
use pagefile::{PageFile, PageNo, PageStore, freelist, overflow};
use vfs::{Mem, Rng, crash_test};

#[test]
fn the_freelist_and_overflow_chains_run_through_the_pool() {
    let mem = Mem::new();
    let file = PageFile::create(&mem, "test.pf", 512).unwrap();
    let pool = BufPool::new(file, 3, Box::new(Clock::new(3)), Box::new(NoLog));
    let mut store = &pool;

    // Ten pages, more than the pool has frames, each with its own stamp.
    let pages: Vec<PageNo> = (0..10)
        .map(|_| freelist::allocate(&mut store).unwrap())
        .collect();
    for &n in &pages {
        pool.fetch_mut(n).unwrap().put_u32(STAMP, n.get());
    }
    freelist::free(&mut store, pages[3]).unwrap();
    freelist::free(&mut store, pages[7]).unwrap();
    // A value that needs 7 overflow pages. It takes the two free pages first.
    let value: Vec<u8> = (0..3000).map(|i| i as u8).collect();
    let first = overflow::write_chain(&mut store, &value).unwrap();
    assert_eq!(overflow::read_chain(&store, first, 3000).unwrap(), value);

    // The header on disk hasn't changed: the pool's copy counts the new pages.
    assert_eq!(PageFile::open(&mem, "test.pf").unwrap().page_count(), 1);
    assert_eq!(pool.header().page_count, 16);

    pool.flush().unwrap();
    let file = PageFile::open(&mem, "test.pf").unwrap();
    assert_eq!(file.page_count(), 16);
    assert_eq!(overflow::read_chain(&file, first, 3000).unwrap(), value);
    let mut page = file.new_page();
    for &n in &pages {
        if n != pages[3] && n != pages[7] {
            file.read(n, &mut page).unwrap();
            assert_eq!(page.u32_at(STAMP), n.get());
        }
    }
    assert_eq!(check(&file).unwrap().problems, Vec::<String>::new());
}

#[test]
fn random_operations_match_the_model() {
    for seed in 1..=20 {
        let mut rng = Rng::new(seed);
        let mem = Mem::new();
        let file = PageFile::create(&mem, "test.pf", 512).unwrap();
        let mut pool = BufPool::new(file, 3, Box::new(Clock::new(3)), Box::new(NoLog));
        // The stamp of every page in use. Freed pages leave the map.
        let mut model: HashMap<PageNo, u32> = HashMap::new();
        for step in 0..500 {
            let mut store = &pool;
            let any = |rng: &mut Rng| *model.keys().nth(rng.below(model.len())).unwrap();
            match rng.below(10) {
                // Allocate a page and stamp it.
                0..=1 => {
                    let n = freelist::allocate(&mut store).unwrap();
                    assert!(
                        !model.contains_key(&n),
                        "seed {seed} step {step}: page {n} twice"
                    );
                    let stamp = rng.next_u64() as u32;
                    pool.fetch_mut(n).unwrap().put_u32(STAMP, stamp);
                    model.insert(n, stamp);
                }
                // Free a page in use.
                2 if !model.is_empty() => {
                    let n = any(&mut rng);
                    freelist::free(&mut store, n).unwrap();
                    model.remove(&n);
                }
                // Change a page's stamp.
                3..=4 if !model.is_empty() => {
                    let n = any(&mut rng);
                    let stamp = rng.next_u64() as u32;
                    pool.fetch_mut(n).unwrap().put_u32(STAMP, stamp);
                    model.insert(n, stamp);
                }
                // Flush, close the file and open it again behind a new pool.
                5 => {
                    pool.flush().unwrap();
                    drop(pool);
                    let file = PageFile::open(&mem, "test.pf").unwrap();
                    pool = BufPool::new(file, 3, Box::new(Clock::new(3)), Box::new(NoLog));
                }
                // Read a page and compare it with the model.
                _ if !model.is_empty() => {
                    let n = any(&mut rng);
                    let stamp = pool.fetch(n).unwrap().u32_at(STAMP);
                    assert_eq!(stamp, model[&n], "seed {seed} step {step}: page {n}");
                }
                _ => {}
            }
        }
        // After a last flush, the file agrees with the model, and `check`
        // finds nothing wrong with it.
        pool.flush().unwrap();
        drop(pool);
        let file = PageFile::open(&mem, "test.pf").unwrap();
        let mut page = file.new_page();
        for (&n, &stamp) in &model {
            file.read(n, &mut page).unwrap();
            assert_eq!(
                page.u32_at(STAMP),
                stamp,
                "seed {seed}: page {n} in the file"
            );
        }
        assert_eq!(
            check(&file).unwrap().problems,
            Vec::<String>::new(),
            "seed {seed}"
        );
    }
}

#[test]
fn the_pools_errors_come_back_through_the_freelist() {
    let mem = Mem::new();
    let pool = pool(&mem, 3, 1);
    // The one frame is pinned, so the new page `allocate` writes has nowhere to go.
    let held = pool.fetch(no(1)).unwrap();
    let mut store = &pool;
    assert!(matches!(
        freelist::allocate(&mut store),
        Err(Error::AllPinned { frames: 1 })
    ));
    drop(held);
    assert_eq!(freelist::allocate(&mut store).unwrap(), no(4));
}

#[test]
fn pages_past_the_page_count_are_refused() {
    let mem = Mem::new();
    let pool = pool(&mem, 3, 2);
    let mut store = &pool;
    // Pages 0 to 3 exist, so page 4 is past the count, for reads and writes.
    let mut page = store.new_page();
    let past = |r| {
        matches!(
            r,
            Err(Error::PageFile(pagefile::Error::OutOfRange {
                page: 4,
                count: 4
            }))
        )
    };
    assert!(past(store.read(no(4), &mut page)));
    assert!(past(store.write(no(4), &mut page)));
    // Once the freelist counts it, it can be written.
    assert_eq!(freelist::allocate(&mut store).unwrap(), no(4));
    store.write(no(4), &mut page).unwrap();
}

/// The stamp of page `n` as it is in the file, not in the pool.
fn stamp_on_disk(mem: &Mem, n: u32) -> u32 {
    let file = PageFile::open(mem, "test.pf").unwrap();
    let mut page = file.new_page();
    file.read(no(n), &mut page).unwrap();
    page.u32_at(STAMP)
}

#[test]
fn a_page_held_for_changing_is_never_written() {
    let mem = Mem::new();
    let pool = pool(&mem, 10, 3);
    let mut held = pool.fetch_mut(no(1)).unwrap();
    held.put_u32(STAMP, 100);
    // Pages 2 to 10 pass through the other two frames. Page 1 stays.
    for n in 2..=10 {
        pool.fetch(no(n)).unwrap();
    }
    assert_eq!(stamp_on_disk(&mem, 1), 1);

    thread::scope(|s| {
        let flusher = s.spawn(|| pool.flush());
        // `flush` waits for the guard. Until it's dropped, the file has the old page.
        thread::sleep(Duration::from_millis(50));
        assert!(!flusher.is_finished());
        assert_eq!(stamp_on_disk(&mem, 1), 1);
        drop(held);
        flusher.join().unwrap().unwrap();
    });
    assert_eq!(stamp_on_disk(&mem, 1), 100);
}

/// What the crash workload was promised.
#[derive(Default)]
struct Acks {
    /// True once `PageFile::create` returned. From then on the file must open.
    created: bool,
    /// The pages that were flushed, with their stamps.
    stamps: Vec<(PageNo, u32)>,
}

/// Turns the pool's errors into the `vfs::Error` that `crash_test` expects.
/// Only file errors, like the power cut, are expected here.
fn op<T>(result: bufpool::Result<T>) -> vfs::Result<T> {
    result.map_err(|e| match e {
        bufpool::Error::PageFile(pagefile::Error::Vfs(e)) => e,
        other => panic!("not a file error: {other}"),
    })
}

/// Creates a file, then twice allocates four pages through a two-frame
/// pool, stamps them and flushes.
fn workload(mem: &Mem, acks: &mut Acks) -> vfs::Result<()> {
    let file = op(PageFile::create(mem, "c.pf", 512).map_err(Into::into))?;
    acks.created = true;
    let pool = BufPool::new(file, 2, Box::new(Clock::new(2)), Box::new(NoLog));
    let mut store = &pool;
    for round in 0..2 {
        let mut stamped = Vec::new();
        for i in 0..4 {
            let n = op(freelist::allocate(&mut store))?;
            op(pool.fetch_mut(n))?.put_u32(STAMP, 100 * round + i);
            stamped.push((n, 100 * round + i));
        }
        op(pool.flush())?;
        acks.stamps.extend(stamped);
    }
    Ok(())
}

/// The file must open after any crash, and every flushed page must have its stamp.
fn flushed_pages_are_there(mem: &Mem, acks: &Acks) -> Result<(), String> {
    if !acks.created {
        return Ok(());
    }
    let file = PageFile::open(mem, "c.pf").map_err(|e| e.to_string())?;
    let mut page = file.new_page();
    for &(n, stamp) in &acks.stamps {
        file.read(n, &mut page).map_err(|e| e.to_string())?;
        if page.u32_at(STAMP) != stamp {
            return Err(format!("page {n} lost its stamp {stamp}"));
        }
    }
    Ok(())
}

#[test]
fn flushed_pages_survive_a_crash() {
    let report = crash_test(50, workload, flushed_pages_are_there);
    println!("{report}");
    // The one failure we expect: the power went off while `flush` was writing
    // the header, and page 0 is torn. A page's stamp is never lost.
    for failure in &report.failures {
        assert!(
            failure.message.starts_with("page 0 is damaged"),
            "{}",
            failure.message
        );
    }
}
