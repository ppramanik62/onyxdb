//! Hit rates of CLOCK, LRU and LRU-2 on the same access traces.
//!
//! 1. The two-pool experiment from the LRU-K paper (section 4.1): references
//!    alternate between 100 hot pages and 10,000 cold ones.
//! 2. A hot set and a long sequential scan, like the paper's Example 1.2.
//! 3. The two-pool trace with every page fetched twice in a row, as a B-tree
//!    does when it reads a leaf and then changes it.
//!
//!     cargo run --release --example hitrate

use bufpool::{BufPool, Clock, LruK, NoLog, Policy};
use pagefile::{PageFile, PageNo, PageStore, freelist};
use vfs::{Mem, Rng};

/// Pool 1 of the two-pool experiment: 100 pages, half of all references.
const N1: u32 = 100;
/// Pool 2: 10,000 pages, the other half.
const N2: u32 = 10_000;
/// How many seeds each result is averaged over.
const SEEDS: u64 = 20;

/// Makes a policy for a pool of the given number of frames.
type MakePolicy = fn(usize) -> Box<dyn Policy>;

/// The policies we compare, by name.
fn policies() -> [(&'static str, MakePolicy); 3] {
    [
        ("CLOCK", |frames| Box::new(Clock::new(frames))),
        ("LRU", |frames| Box::new(LruK::new(frames, 1))),
        ("LRU-2", |frames| Box::new(LruK::new(frames, 2))),
    ]
}

/// Creates `trace.pf` in `mem` with `pages` empty pages.
fn make_file(mem: &Mem, pages: u32) {
    let mut file = PageFile::create(mem, "trace.pf", 512).unwrap();
    let mut page = file.new_page();
    for _ in 0..pages {
        let n = freelist::allocate(&mut file).unwrap();
        file.write(n, &mut page).unwrap();
    }
    file.sync().unwrap();
}

/// Runs `warm` references and then `measured` more from `next` through a
/// pool of `frames` frames, and returns the hit rate of the measured ones.
fn hit_rate(
    mem: &Mem,
    frames: usize,
    policy: MakePolicy,
    warm: usize,
    measured: usize,
    mut next: impl FnMut() -> u32,
) -> f64 {
    let file = PageFile::open(mem, "trace.pf").unwrap();
    let pool = BufPool::new(file, frames, policy, Box::new(NoLog));
    for _ in 0..warm {
        pool.fetch(PageNo::new(next()).unwrap()).unwrap();
    }
    let before = pool.stats();
    for _ in 0..measured {
        pool.fetch(PageNo::new(next()).unwrap()).unwrap();
    }
    let after = pool.stats();
    (after.hits - before.hits) as f64 / measured as f64
}

/// The two-pool reference string: odd steps pick a page from pool 1, even
/// steps from pool 2. With `twice`, every page comes twice in a row.
fn two_pool(seed: u64, twice: bool) -> impl FnMut() -> u32 {
    let mut rng = Rng::new(seed);
    let mut step = 0u64;
    let mut last = 0;
    move || {
        step += 1;
        if twice && step.is_multiple_of(2) {
            return last;
        }
        let pick = if twice { step / 2 } else { step };
        last = if pick.is_multiple_of(2) {
            1 + rng.below(N1 as usize) as u32
        } else {
            1 + N1 + rng.below(N2 as usize) as u32
        };
        last
    }
}

fn main() {
    let mem = Mem::new();
    make_file(&mem, N1 + N2);

    println!("1. Two pools: 100 and 10,000 pages, alternating.");
    println!("   1,000 references to warm up, 3,000 measured, {SEEDS} seeds.");
    println!("   The paper's Table 4.1 is in brackets.");
    println!("     B    CLOCK   LRU            LRU-2");
    let paper = [
        (60, 0.14, 0.291),
        (80, 0.18, 0.382),
        (100, 0.22, 0.459),
        (120, 0.26, 0.496),
        (140, 0.29, 0.502),
        (160, 0.32, 0.503),
        (180, 0.34, 0.504),
        (200, 0.37, 0.505),
        (250, 0.42, 0.508),
        (300, 0.45, 0.510),
        (350, 0.48, 0.513),
        (400, 0.49, 0.515),
        (450, 0.50, 0.517),
    ];
    for (frames, lru1, lru2) in paper {
        let mut rates = Vec::new();
        for (_, make) in policies() {
            let total: f64 = (1..=SEEDS)
                .map(|seed| hit_rate(&mem, frames, make, 1000, 3000, two_pool(seed, false)))
                .sum();
            rates.push(total / SEEDS as f64);
        }
        println!(
            "   {frames:>3}   {:.3}   {:.3} [{lru1:.2}]   {:.3} [{lru2:.3}]",
            rates[0], rates[1], rates[2]
        );
    }

    println!();
    println!("2. A hot set and a scan: 100 hot pages and 10,000 others, 120 frames.");
    println!("   80% of references go to a random hot page, 20% to the next page of");
    println!("   a scan through the others. 20,000 references to warm up, 20,000 measured.");
    for (name, make) in policies() {
        let total: f64 = (1..=SEEDS)
            .map(|seed| {
                let mut rng = Rng::new(seed);
                let mut scan = 0;
                let next = move || {
                    if rng.below(5) < 4 {
                        1 + rng.below(N1 as usize) as u32
                    } else {
                        scan = (scan + 1) % N2;
                        1 + N1 + scan
                    }
                };
                hit_rate(&mem, 120, make, 20_000, 20_000, next)
            })
            .sum();
        println!(
            "   {name:<6} {:.3}   (the best possible is 0.800)",
            total / SEEDS as f64
        );
    }

    println!();
    println!("3. Two pools again, 100 frames, but every page is fetched twice in a row.");
    println!("   Hit rates count both fetches, so every second fetch is a hit.");
    for (name, make) in policies() {
        let once: f64 = (1..=SEEDS)
            .map(|seed| hit_rate(&mem, 100, make, 1000, 3000, two_pool(seed, false)))
            .sum();
        let twice: f64 = (1..=SEEDS)
            .map(|seed| hit_rate(&mem, 100, make, 2000, 6000, two_pool(seed, true)))
            .sum();
        println!(
            "   {name:<6} once {:.3}   twice {:.3}, or {:.3} on the first fetch of each pair",
            once / SEEDS as f64,
            twice / SEEDS as f64,
            2.0 * twice / SEEDS as f64 - 1.0
        );
    }
}
