# bufpool

A buffer pool: a fixed number of page-sized frames in memory, in front of a [`pagefile`](../pagefile).

```text
fetch(n) ──▶ page table ──▶ frame ──▶ guard: page n, pinned and latched
                 │ miss
                 ▼
     an empty frame, or a victim the policy picks
     (a dirty victim is written back first, after the log hook says
      the log is durable up to the page's LSN)
```

Every read and write of a page goes through the pool, so the pool holds the only
copy of a page that is being changed. Code uses a page through a guard. A guard
pins its frame, so the page stays in memory. It also latches the frame, so no
other thread changes the page while the guard exists.

## The public API

| Item | What it is | When to use it |
|---|---|---|
| `BufPool` | the pool: `new`, `fetch`, `fetch_mut`, `header`, `header_mut`, `flush`, `stats` | to read or change pages of a `PageFile` |
| `PageRef`, `PageMut` | guards that deref to a `Page`. `PageRef` holds a shared latch, and `PageMut` an exclusive one | `fetch` and `fetch_mut` return them |
| `HeaderRef`, `HeaderMut` | guards on the pool's copy of the header | to read or change the page count, the freelist or the user bytes |
| `PageStore for &BufPool` | `pagefile`'s trait, implemented by the pool | to run the freelist or overflow chains through the pool |
| `Policy` | a trait: `touch`, `victim`, `forget` | to write a new eviction policy |
| `Clock`, `LruK` | the two policies | to make a pool |
| `LogHook`, `NoLog` | a trait with one call, `flush_to(lsn)`, and a hook for no log | to connect the pool to a log, or to test without one |
| `PageTable<V>` | a fixed-size hash table from `PageNo` to `V` | to find which frame holds a page |
| `Stats` | `hits`, `misses`, `evictions`, `writes`, and `hit_rate()` | to measure how well the pool works |
| `Error`, `Result` | `PageFile`, `AllPinned`, `Log` | every call that can fail returns them |

## Use

```rust
use bufpool::{BufPool, Clock, NoLog};
use pagefile::{PageFile, PageStore, freelist};
use vfs::Mem;

let mem = Mem::new();
let file = PageFile::create(&mem, "db.pf", 4096).unwrap();
let pool = BufPool::new(file, 8, Box::new(Clock::new(8)), Box::new(NoLog));

// The freelist runs through the pool, so the new page and the header
// change in memory first.
let mut store = &pool;
let n = freelist::allocate(&mut store).unwrap();

// Change the page. The guard holds the frame's latch until it is dropped.
let mut page = pool.fetch_mut(n).unwrap();
page.set_lsn(7);
drop(page);

// Write the dirty pages, then the header, and sync.
pool.flush().unwrap();
drop(pool);

let file = PageFile::open(&mem, "db.pf").unwrap();
let mut page = file.new_page();
file.read(n, &mut page).unwrap();
assert_eq!(page.lsn(), 7);
```

## Guards and threads

```text
                     pin    latch       other threads can
fetch(n)     PageRef  yes   shared      fetch(n), not fetch_mut(n)
fetch_mut(n) PageMut  yes   exclusive   neither, until the guard is dropped
drop(guard)           unpins and releases the latch
```

- `fetch_mut` marks the page dirty immediately. Set the page's LSN before the
  guard is dropped.
- A thread that holds a guard for page `n` and calls `fetch_mut(n)` waits
  forever.
- When every frame is pinned, a miss returns `AllPinned`. A thread must never
  hold more guards at the same time than the pool has frames.

## Writing back

```text
evicting a dirty page           flush()
  log.flush_to(page LSN)          every dirty page: log.flush_to, write
  write the page                  sync
  reuse the frame                 log.flush_to(u64::MAX), write the header
                                  sync
```

The pool keeps its own copy of the header and never evicts it. `flush` writes
the header last, so the header on disk never counts a page that the file does
not have.

## Policies

| Policy | Evicts | Cost |
|---|---|---|
| `Clock::new(frames)` | the first frame whose usage count is 0. Each fetch adds 1, up to 5. The hand takes 1 from each frame it passes | one byte per frame. It only approximates least recently used, and one big scan can evict the pages used most |
| `LruK::new(frames, k)` | the page whose K-th most recent fetch is oldest. A page fetched fewer than K times is evicted first | reads every frame to find a victim, and remembers up to 8 evicted pages per frame |

```text
LRU-2     fetched at     2nd newest
page 7    40  31         31   evicted last
page 3    39             -    evicted first: fetched once
page 9    38  12         12   evicted second
```

## Known gaps

- A crash while `flush` writes the header can tear page 0, and
  `PageFile::open` then refuses the file. The crash test hits this in 48 of 950
  crashes. Pages that `flush` wrote are never lost.
- Hits do not scale across threads, because of the one mutex. Splitting the
  page table into parts, as PostgreSQL does, would fix this.
- After its guard is dropped, a changed page can be evicted and written at any
  time, even if the change it belongs to is not finished. So the code above the
  pool must hold its guards until the change is safe in the log.

## Tests, examples and benchmarks

```text
cargo test -p bufpool
cargo bench -p bufpool
cargo run --release -p bufpool --example table     how pages fill a small page table, and what removing one does
cargo run --release -p bufpool --example hitrate   hit rates of CLOCK, LRU and LRU-2 on the same traces
```

| Test | What it checks |
|---|---|
| `tests/table.rs` | 50 seeds of 2000 random operations against a `HashMap`, and removing from the middle of a run |
| `tests/pool.rs` | hits, eviction when the pool is full, pinned pages are not evicted, and a damaged page is caught when it is loaded |
| `tests/dirty.rs` | a change survives eviction, a clean page is never written, and no page reaches the file before its log record |
| `tests/store.rs` | the freelist and overflow chains through the pool, 20 seeds of 500 random operations against a model, page count checks, and a crash after every write of a flush workload |
| `tests/lruk.rs` | after one scan of 100 pages, LRU keeps none of 5 hot pages and LRU-2 keeps all 5 |
| `tests/threads.rs` | 8 threads against a model, threads that miss one page share one read, a failed read fails every waiting thread, and a page that is being written back can still be fetched |

`benches/pool.rs` prints fetches per second for hits and misses, with 1 and 8
threads, on a file system in memory. The baseline is saved in
`benches/baseline.txt`, with the machine it ran on:

```text
# 2026-11-24 · Intel Core i5-13500H, 16 threads · WD SN740 NVMe, btrfs · rustc 1.98.1
4 KiB pages, 400000 random fetches per thread, CLOCK, in memory
hits,   1 thread(s):   17942656 fetches/s
misses, 1 thread(s):     140739 fetches/s
hits,   8 thread(s):    2303075 fetches/s
misses, 8 thread(s):     867954 fetches/s
```

The references behind this crate are in [engineering-notes](https://github.com/PPRAMANIK62/engineering-notes/blob/main/database-references/README.md#buffer-manager).
