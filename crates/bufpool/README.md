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

Every read and write of a page goes through the pool, so it holds the only copy of a page that's being changed. Pages are used through guards. A guard pins its frame, so the page stays in memory, and latches it, so no other thread changes the page underneath.

## The public API

| Item | What it is | Use it when |
|---|---|---|
| `BufPool` | the pool: `new`, `fetch`, `fetch_mut`, `header`, `header_mut`, `flush`, `stats` | you read or change pages of a `PageFile` |
| `PageRef`, `PageMut` | guards that deref to a `Page`. `PageRef` holds a shared latch, `PageMut` an exclusive one | you got a page from `fetch` or `fetch_mut` |
| `HeaderRef`, `HeaderMut` | guards on the pool's copy of the header | you read or change the page count, the freelist or the user bytes |
| `PageStore for &BufPool` | `pagefile`'s trait, implemented by the pool | you run the freelist or overflow chains through the pool |
| `Policy` | a trait: `touch`, `victim`, `forget` | you write your own eviction policy |
| `Clock`, `LruK` | the two policies | you make a pool |
| `LogHook`, `NoLog` | a trait with one call, `flush_to(lsn)`, and a hook for no log | you connect the pool to a log, or test without one |
| `PageTable<V>` | a fixed-size hash table from `PageNo` to `V` | you need to find which frame holds a page |
| `Stats` | `hits`, `misses`, `evictions`, `writes`, and `hit_rate()` | you want to know how well the pool is doing |
| `Error`, `Result` | `PageFile`, `AllPinned`, `Log` | every fallible call returns them |

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

// Change the page. The guard holds the frame's latch until it's dropped.
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
fetch_mut(n) PageMut  yes   exclusive   neither, until it's dropped
drop(guard)           unpins and lets go of the latch
```

- `fetch_mut` marks the page dirty right away. Set the page's LSN before dropping the guard.
- A thread that holds a guard for page `n` and calls `fetch_mut(n)` waits forever.
- When every frame is pinned, a miss returns `AllPinned`. Hold at most as many guards at once as the pool has frames.

## Writing back

```text
evicting a dirty page           flush()
  log.flush_to(page LSN)          every dirty page: log.flush_to, write
  write the page                  sync
  reuse the frame                 log.flush_to(u64::MAX), write the header
                                  sync
```

The pool keeps its own copy of the header and never evicts it. `flush` writes it last, so the header on disk never counts a page the file doesn't have.

## Policies

| Policy | Evicts | Cost |
|---|---|---|
| `Clock::new(frames)` | the first frame whose usage count is 0. Each fetch adds 1, up to 5. The hand takes 1 off each frame it passes | one byte per frame. Only approximates least recently used, and one big scan can push out the pages used most |
| `LruK::new(frames, k)` | the page whose K-th most recent fetch is oldest. A page fetched fewer than K times goes first | looks at every frame to find a victim, and remembers up to 8 evicted pages per frame |

```text
LRU-2     fetched at     2nd newest
page 7    40  31         31   evicted last
page 3    39             -    evicted first: fetched once
page 9    38  12         12   evicted second
```

## Decisions

- **The number of frames is fixed at `new`.**
  Why: memory for pages has a hard limit, and no fetch allocates.
  Cost: the pool can't grow. Work that needs more pages at once than there are frames gets `AllPinned`.
- **`AllPinned` is an error, not a wait.**
  Why: a thread that holds every pin itself would wait forever.
  Cost: the caller has to handle it.
- **Pages are used only through guards.**
  Why: dropping the guard unpins and unlatches, so a caller can't forget, even on an early return or a `?`.
  Cost: fetching a page for changing while holding it in the same thread deadlocks.
- **A pin and a latch are separate.**
  Why: the pin keeps the page in its frame, and the latch keeps its bytes from changing. A thread that waits for a latch already holds a pin, so the page can't be evicted while it waits.
  Cost: two things to take on every fetch.
- **One mutex over the page table, pins, dirty flags and policy. One `RwLock` per frame for the bytes.**
  Why: a fetch changes all the shared state together, and every step under the mutex is a few memory reads and writes. Work on a page can take long, so each frame has its own latch.
  Cost: hits don't scale across threads. They all wait on the one mutex.
- **No I/O while holding the mutex.**
  Why: a disk read takes thousands of times longer than the work the mutex protects. One miss would stall every hit.
  Cost: threads that miss the same page must wait for one shared read, and a failed read fails all of them.
- **`fetch_mut` marks the page dirty at once.**
  Why: a page held for changing is always dirty, and dropping the guard only lets go.
  Cost: one needless write if the caller changes nothing.
- **The write-ahead rule goes through a trait, `LogHook`.**
  Why: the pool only needs "the log is durable up to here", not how the log works.
  Cost: one call before every page write. `NoLog` stands in when there's no log.
- **The pool keeps its own header. `flush` writes pages, syncs, then writes the header and syncs again.**
  Why: allocating doesn't write the header each time, and the header on disk never counts a page the file doesn't have.
  Cost: a crash while the header is being written can tear page 0. See the gaps.
- **`PageStore` is implemented for `&BufPool`.**
  Why: the freelist and overflow chains change pages and the header in memory first, like any other change.
  Cost: a sequence of calls isn't atomic. The code above has to group them.
- **`PageStore::write` doesn't read the page first.**
  Why: the caller replaces the whole page, so the read would be wasted. It also means a torn page can be rewritten.
  Cost: none.
- **`fetch` doesn't check the page count. `PageStore` reads and writes do.**
  Why: most callers know the page exists, and the check takes the header's latch.
  Cost: a `fetch` past the end of the file fails with a short read, not a page count error.
- **CLOCK is the default policy.**
  Why: a fetch only adds 1 to a byte, and every fetch runs the policy under the mutex. The usage count is capped at 5, like PostgreSQL's clock sweep.
  Cost: one scan can flush the pool. `LruK` is the choice when that matters.
- **The policy never sees pins. The pool passes in which frames can be evicted.**
  Why: pins live in one place, so a policy bug can't evict a page someone is using.
  Cost: `victim` takes a closure.
- **The page table is our own hash table: open addressing with linear probing, twice as many slots as frames.**
  Why: one flat array with no allocation after `new`, and never more than half full, so runs stay short.
  Cost: removing an entry moves later entries of its run back. Half the slots are always empty.

## Known gaps

- A crash while `flush` writes the header can tear page 0, and `PageFile::open` then refuses the file. The crash test hits this in 48 of 950 crashes. Flushed pages themselves are never lost.
- Hits don't scale across threads, because of the one mutex. Splitting the page table into parts, as PostgreSQL does, would fix it.
- Once its guard is dropped, a changed page can be evicted and written at any time, even if the change it's part of isn't finished. The code above the pool has to hold its guards until the change is safe in the log.

## Tests and benchmarks

```text
cargo test -p bufpool
cargo bench -p bufpool
```

| Test | What it checks |
|---|---|
| `tests/table.rs` | 50 seeds of 2000 random operations against a `HashMap`, and removing from the middle of a run |
| `tests/pool.rs` | hits, eviction when full, pinned pages stay, and a damaged page is caught when it's loaded |
| `tests/dirty.rs` | a change survives eviction, a clean page is never written, and no page reaches the file before its log |
| `tests/store.rs` | the freelist and overflow chains through the pool, 20 seeds of 500 random operations against a model, page count checks, and a crash after every write of a flush workload |
| `tests/lruk.rs` | after one scan of 100 pages, LRU keeps none of 5 hot pages and LRU-2 keeps all 5 |
| `tests/threads.rs` | 8 threads against a model, threads that miss one page share one read, a failed read fails every waiting thread, and a page being written back can still be fetched |

`benches/pool.rs` prints fetches per second for hits and misses, with 1 and 8 threads. Save your first run to `benches/baseline.txt` and compare later runs against it.

The references behind this crate are in [engineering-notes](https://github.com/PPRAMANIK62/engineering-notes/blob/main/database-references/README.md#buffer-manager).
