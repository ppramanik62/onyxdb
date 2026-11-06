//! Fetches per second, for hits and for misses, with 1 and 8 threads.
//! Standard library only: it times a fixed number of fetches with `Instant`.
//!
//!     cargo bench -p bufpool --bench pool
//!
//! `baseline.txt` beside this file holds the numbers from an earlier run.
//! A change that makes a number worse says so in its commit.

use std::thread;
use std::time::Instant;

use bufpool::{BufPool, Clock, NoLog};
use pagefile::{PageFile, PageNo, PageStore, freelist};
use vfs::{Mem, Rng};

/// Fetches each thread makes.
const FETCHES: usize = 400_000;

/// A pool of `frames` frames over a new file of `pages` pages in `mem`.
fn pool(mem: &Mem, pages: u32, frames: usize) -> BufPool {
    let mut file = PageFile::create(mem, "bench.pf", 4096).unwrap();
    let mut page = file.new_page();
    for _ in 0..pages {
        let n = freelist::allocate(&mut file).unwrap();
        file.write(n, &mut page).unwrap();
    }
    BufPool::new(file, frames, Box::new(Clock::new(frames)), Box::new(NoLog))
}

/// Runs `threads` threads that each fetch `FETCHES` random pages from
/// 1 to `pages`, and returns fetches per second over all of them.
fn run(pool: &BufPool, pages: u32, threads: u64) -> f64 {
    let start = Instant::now();
    thread::scope(|s| {
        for t in 0..threads {
            s.spawn(move || {
                let mut rng = Rng::new(t + 1);
                for _ in 0..FETCHES {
                    let n = PageNo::new(1 + rng.below(pages as usize) as u32).unwrap();
                    pool.fetch(n).unwrap();
                }
            });
        }
    });
    (FETCHES as u64 * threads) as f64 / start.elapsed().as_secs_f64()
}

fn main() {
    let hits = pool(&Mem::new(), 64, 64);
    let misses = pool(&Mem::new(), 1000, 16);
    println!("4 KiB pages, {FETCHES} random fetches per thread, CLOCK, in memory");
    for threads in [1, 8] {
        println!(
            "hits,   {threads} thread(s): {:>10.0} fetches/s",
            run(&hits, 64, threads)
        );
        println!(
            "misses, {threads} thread(s): {:>10.0} fetches/s",
            run(&misses, 1000, threads)
        );
    }
    println!("{:?}", misses.stats());
}
