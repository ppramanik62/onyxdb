//! `BufPool`: a fixed number of frames in memory, each holding one page of
//! a page file.
//!
//! | Term | Meaning |
//! |---|---|
//! | frame | One page-sized buffer. Frames are numbered from 0 and never move. |
//! | pin | A count of the guards and threads using a frame. A frame with pins is never emptied. |
//! | latch | A frame's `RwLock`. Shared to read the page, exclusive to change it. |
//! | guard | A [`PageRef`] or [`PageMut`]. It holds one pin and the latch until it's dropped. |
//! | dirty | The page in the frame changed since it was read or last written. |
//! | claim | Give an empty or evicted frame to a new page and load the page into it. |
//! | write back | Write a dirty page to the file, after the log hook says the log is durable. |
//!
//! A caller fetches a page by number and gets a guard. While the guard lives, the
//! frame is pinned, so the page stays, and latched, so no other thread changes
//! it underneath. A `PageRef` holds a shared latch, and a `PageMut` an exclusive one.
//!
//! | page table | frames |
//! | --- | --- |
//! | page 7, frame 2 | frame 0: page 3, pins 0, clean |
//! | page 3, frame 0 | frame 1: empty |
//! | | frame 2: page 7, pins 1, dirty |
//!
//! A fetch of page 7 here is a hit: it pins frame 2 and takes its latch.
//! A fetch of page 5 is a miss: it takes the empty frame 1 and reads page 5 into it.
//! Once no frame is empty, the policy picks a frame with no pins, and that frame's
//! page is written back first if it's dirty.
//!
//! Three rules keep this safe with many threads:
//!
//! 1. A page is used only through a guard. A guard lets go of its latch
//!    before its pin.
//! 2. Pins and dirty flags change only under the pool's mutex. A page's bytes
//!    change only under its exclusive latch, and the dirty flag is set while
//!    that latch is held.
//! 3. No thread waits for I/O while it holds the mutex. The only latch held
//!    during I/O is the latch of the frame being read or written.
//!
//! Why it's built this way:
//!
//! - One mutex covers the page table, the pins, the dirty flags and the policy,
//!   because a fetch changes all of them together. With one mutex, a fetch sees
//!   them to agree. Every step under it is a few memory reads and writes, so
//!   threads wait on it only briefly.
//! - Each frame's bytes have their own latch instead, because work on a page can
//!   take long: a caller holds a guard while it reads or changes the page. One
//!   latch per frame lets threads work on different pages at the same time, and
//!   lets many threads read one page at once.
//! - A pin and a latch are separate because they protect different things. The
//!   pin keeps the page in its frame. The latch keeps the bytes from changing under a reader.
//!   A thread that waits for a latch already holds a pin, so the page can't be
//!   evicted while it waits.
//! - I/O is never done under the mutex, because a disk read takes thousands of
//!   times longer than the work mutex protects. If one miss held the mutex, every
//!   other fetch, even a hit, would wait for that disk read.
//! - Pages are used only through guards, so the pin and the latch are let go
//!   when the guard is dropped. A caller can't forget to unpin, even on an early
//!   return or a `?`.

use std::{
    ops::Deref,
    sync::{Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard},
};

use pagefile::{Header, Page, PageFile, PageNo, PageStore};

use crate::{Error, LogHook, PageTable, Policy, Result};

/// A buffer pool in front of one page file.
///
/// It holds a fixed number of frames, each with room for one page. Pages are read
/// into frames on demand and stay there until the policy evicts them.
/// It's `Send` and `Sync`, so threads share it by reference, for example with
/// `std::thread::scope`.
///
/// The pool takes the [`PageFile`] by value, so every read and write of the file
/// goes through the pool. Don't open the same file again elsewhere while the pool
/// is in front of it, or the two copies of each page drift apart.
///
/// # Examples
///
/// ```
/// # use bufpool::{BufPool, Clock, NoLog};
/// # use pagefile::{PageFile, PageNo, PageStore, freelist};
/// # use vfs::Mem;
/// # let mem = Mem::new();
/// # let mut file = PageFile::create(&mem, "db", 512).unwrap();
/// # for _ in 0..3 {
/// #     let n = freelist::allocate(&mut file).unwrap();
/// #     file.write(n, &mut file.new_page()).unwrap();
/// # }
/// // A page file with pages 1 to 3, and a pool of 2 frames in front of it.
/// let pool = BufPool::new(file, 2, Box::new(Clock::new(2)), Box::new(NoLog));
/// let one = PageNo::new(1).unwrap();
/// let page = pool.fetch(one).unwrap();
/// assert_eq!(page.number(), one);
/// assert_eq!(page.lsn(), 0);
/// drop(page);
///
/// // Page 1 is still in its frame, so the second fetch is a hit.
/// pool.fetch(one).unwrap();
/// assert_eq!((pool.stats().hits, pool.stats().misses), (1, 1));
/// ```
pub struct BufPool {
    /// The page file behind the pool. Only the pool reads and writes it.
    pub(crate) file: PageFile,
    /// The size of every page in bytes.
    pub(crate) page_size: usize,
    /// One page per frame. Each lock is that frame's latch.
    frames: Vec<RwLock<Page>>,
    /// Everything about the frames except their bytes, behind one mutex.
    state: Mutex<State>,
    /// The pool's copy of the header, with its own latch. It's never evicted.
    ///
    /// It isn't kept in a frame because it's read on every allocation and free,
    /// so it would never be a good victim anyway. Kept apart, it never takes a
    /// frame from the pages, and it's a decoded `Header`, not bytes that every
    /// reader would have to decode again.
    pub(crate) header: RwLock<Header>,
    /// Called before a page is written, to keep the write-ahead rule.
    pub(crate) log: Box<dyn LogHook>,
}

/// Everything the pool's mutex protects: which page is in which frame, the pins
/// and dirty flags, and the policy. A frame's bytes aren't in here. They're behind
/// the frame's own latch.
pub(crate) struct State {
    /// Which frame holds each page in the pool.
    table: PageTable<usize>,
    /// One entry per frame.
    meta: Vec<Meta>,
    /// Frames that hold no page. They're used before the policy is asked.
    empty: Vec<usize>,
    /// Picks which frame to empty when the pool needs room.
    policy: Box<dyn Policy>,
    /// The counts that `stats` returns.
    stats: Stats,
    pub(crate) header_dirty: bool,
}

/// What the pool knows about one frame.
#[derive(Clone, Copy)]
struct Meta {
    /// The page in the frame, or `None` for an empty frame.
    page: Option<PageNo>,
    /// How many guards, and threads in the middle of an operation, hold the
    /// frame. The frame is emptied only when this hits 0.
    pins: u32,
    /// True if the page changed since it was read or last written.
    dirty: bool,
}

impl Meta {
    /// A frame with no page in it.
    const EMPTY: Meta = Meta {
        page: None,
        pins: 0,
        dirty: false,
    };
}

/// Counts of what the pool has done since it was made.
///
/// A fetch is either a hit or a miss. A miss that had to empty a frame that held
/// another page also counts an eviction.
///
/// # Examples
///
/// ```
/// use bufpool::Stats;
///
/// let stats = Stats { hits: 3, misses: 1, ..Stats::default() };
/// assert_eq!(stats.hit_rate(), 0.75);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stats {
    /// Fetches that found the page in a frame.
    pub hits: u64,
    /// Fetches that had to load the page.
    pub misses: u64,
    /// Pages removed from a frame to make room for another.
    pub evictions: u64,
    /// Pages written to the file, by eviction or by flushing. The header
    /// isn't counted.
    pub writes: u64,
}

impl Stats {
    /// Hits as a share of all fetches, from 0 to 1. It's 0 when there have been
    /// no fetches.
    pub fn hit_rate(&self) -> f64 {
        self.hits as f64 / (self.hits + self.misses).max(1) as f64
    }
}

impl BufPool {
    /// Makes a pool of `frames` frames in front of `file`. Every frame starts
    /// empty.
    ///
    /// `policy` picks the frames to empty, and must be made for the same number
    /// of frames. `log` is called before any page write, to keep the write-ahead
    /// rule. The pool copies the file's header and keeps its own copy from then on.
    ///
    /// This allocates all the frames, `frames` times the page size, up front.
    ///
    /// # Panics
    ///
    /// Panics if `frames` is 0.
    pub fn new(
        file: PageFile,
        frames: usize,
        policy: Box<dyn Policy>,
        log: Box<dyn LogHook>,
    ) -> BufPool {
        assert!(frames > 0, "a pool needs at least one frame");
        let page_size = file.page_size();
        let header = file.header().clone();
        BufPool {
            file,
            page_size,
            frames: (0..frames)
                .map(|_| RwLock::new(Page::new(page_size)))
                .collect(),
            state: Mutex::new(State {
                table: PageTable::new(frames),
                meta: vec![Meta::EMPTY; frames],
                // Reversed, so that `pop` hands out frame 0 first.
                empty: (0..frames).rev().collect(),
                policy,
                stats: Stats::default(),
                header_dirty: false,
            }),
            header: RwLock::new(header),
            log,
        }
    }

    /// A copy of the pool's counts so far. The counts keep changing after this
    /// returns.
    pub fn stats(&self) -> Stats {
        self.lock().stats
    }

    /// Takes the pool's mutex. Callers keep it only for quick work on the state,
    /// never while they wait for I/O or for a latch.
    ///
    /// # Panics
    ///
    /// Panics if a thread panicked while it held the mutex, because the pins and
    /// the page table may then be wrong.
    pub(crate) fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .expect("a thread panicked while holding the pool's mutex")
    }

    /// Takes the shared latch of `frame`, waiting while another thread
    /// holds its exclusive latch. The caller must have pinned the frame.
    fn read_latch(&self, frame: usize) -> RwLockReadGuard<'_, Page> {
        self.frames[frame]
            .read()
            .expect("a thread panicked while changing a page")
    }

    /// Takes the exclusive latch of `frame`, waiting until no other thread
    /// holds its latch at all. The caller must have pinned the frame.
    pub(crate) fn write_latch(&self, frame: usize) -> RwLockWriteGuard<'_, Page> {
        self.frames[frame]
            .write()
            .expect("a thread panicked while changing a page")
    }
}

/// What `pin` returns: the frame's index, and the frame's exclusive latch, if
/// this thread just claimed the frame for its page. A frame that already held
/// the page comes with no latch, and the caller takes the one it needs.
///
/// The `'a` ties the latch to the pool it came from, so the latch can't outlive
/// the pool.
pub(crate) type Pinned<'a> = (usize, Option<RwLockWriteGuard<'a, Page>>);

impl BufPool {
    /// Pins the frame that holds page `n`. If the page isn't in the pool, it
    /// claims the frame for it first: an empty one if there is one, or else
    /// the policy's victim, written back first if it's dirty.
    ///
    /// With `read` false, a claimed frame is zeroed instead of read from the
    /// file, for a caller that will overwrite the whole page.
    ///
    /// The caller must call `unpin` once when it's done with the frame, and
    /// must let go of the latch, if it got one, before that.
    ///
    /// # Errors
    ///
    /// [`Error::AllPinned`] if no frame can be emptied, and any error from
    /// writing back the victim or reading page `n`.
    pub(crate) fn pin(&self, n: PageNo, read: bool) -> Result<Pinned<'_>> {
        let mut state = self.lock();
        loop {
            if let Some(frame) = state.table.get(n) {
                state.meta[frame].pins += 1;
                state.policy.touch(frame, n);
                state.stats.hits += 1;
                return Ok((frame, None));
            }
            let frame = match state.empty.pop() {
                Some(frame) => frame,
                None => {
                    let State { policy, meta, .. } = &mut *state;
                    let evictable = |f: usize| meta[f].pins == 0 && meta[f].page.is_some();
                    policy.victim(&evictable).ok_or(Error::AllPinned {
                        frames: self.frames.len(),
                    })?
                }
            };
            if state.meta[frame].dirty {
                // Write the victim back first, without the mutex. Our pin keeps
                // any other thread from choosing it meanwhile.
                state.meta[frame].pins += 1;
                drop(state);
                let written = self.write_back(frame);
                state = self.lock();
                state.meta[frame].pins -= 1;
                written?;
                // While the mutex was free, another thread may have pinned or
                // changed the victim, or loaded page `n` itself. If so, start over.
                let meta = state.meta[frame];
                if meta.pins > 0 || meta.dirty || state.table.get(n).is_some() {
                    continue;
                }
            }
            return self.claim(state, frame, n, read);
        }
    }

    fn write_back(&self, _frame: usize) -> Result<()> {
        Ok(())
    }

    /// Gies `frame` to page `n` and loads the page into it. `frame` must be empty
    /// or hold a clean page with no pins, and the caller passes in the mutex guard
    /// it checked that under.
    ///
    /// Returns with the frame pinned once and its exclusive latch held. The mutex
    /// is let go before the read, so other threads can go on working.
    ///
    /// # Errors
    ///
    /// Any error from reading the page. The frame is then emptied, and it goes back
    /// on the empty list once its last pin is gone.
    fn claim(
        &self,
        mut state: MutexGuard<'_, State>,
        frame: usize,
        n: PageNo,
        read: bool,
    ) -> Result<Pinned<'_>> {
        if let Some(old) = state.meta[frame].page {
            state.table.remove(old);
            state.policy.forget(frame);
            state.stats.evictions += 1;
        }
        state.meta[frame] = Meta {
            page: Some(n),
            pins: 1,
            dirty: false,
        };
        state.table.insert(n, frame);
        state.policy.touch(frame, n);
        state.stats.misses += 1;
        // A latch is only held inside a guard, and a guard holds a pin. The frame
        // had no pins, so nobody holds its latch and this never waits.
        let mut latch = self.frames[frame]
            .try_write()
            .expect("a frame with no pins has no latch holders");
        // Other threads that want page `n` now find it in the table, pin the frame,
        // and wait on the latch until the page is loaded. So a page that many
        // thread miss at once is read from the file only once.
        drop(state);
        if !read {
            latch.bytes_mut().fill(0);
        } else if let Err(e) = self.file.read_page(n.get(), &mut latch) {
            let mut state = self.lock();
            state.table.remove(n);
            state.policy.forget(frame);
            state.meta[frame].page = None;
            drop(state);
            drop(latch);
            self.unpin(frame);
            return Err(e.into());
        }
        Ok((frame, Some(latch)))
    }

    /// Takes away one pin from `frame`. The caller must hold a pin on it and must
    /// have let go of the frame's latch first.
    pub(crate) fn unpin(&self, frame: usize) {
        let mut state = self.lock();
        let meta = &mut state.meta[frame];
        meta.pins -= 1;
        // A frame that failed to load holds no page. The last thread to let go
        // of it puts it back on the empty list.
        if meta.pins == 0 && meta.page.is_none() {
            state.empty.push(frame);
        }
    }

    /// True if `frame` still holds page `n`. A thread that waited on the latch
    /// checks this, because the load it waited for may have failed.
    fn holds(&self, frame: usize, n: PageNo) -> bool {
        self.lock().meta[frame].page == Some(n)
    }

    /// Fetches page `n` for reading, and returns a guard that derefs to the
    /// [`Page`].
    ///
    /// If the page isn't in the pool, it's read from the file and its checksum
    /// is checked. If no frame is empty, the policy picks a frame with no pins
    /// to reuse, and that frame's page is written back first if it's dirty.
    ///
    /// While the guard lives, the page stays in its frame. Other threads can read
    /// the page at the same time, but no thread can change it.
    ///
    /// # Errors
    ///
    /// - [`Error::AllPinned`] if the page has to be loaded and every frame holds
    ///   a pinned page already.
    /// - [`Error::PageFile`] if reading the page fails or its checksum is wrong,
    /// or if writing back the evicted page fails.
    /// - [`Error::Log`] if the log can't be made durable before that write.
    ///
    /// # Deadlocks
    ///
    /// Blocks while another thread holds a [`PageMut`] for the same page. So a
    /// thread that holds a `PageMut` for page `n` and calls `fetch(n)` waits
    /// forever.
    ///
    /// # Examples
    ///
    /// ```
    /// # use bufpool::Error;
    /// # use bufpool::{BufPool, Clock, NoLog};
    /// # use pagefile::{PageFile, PageNo, PageStore, freelist};
    /// # use vfs::Mem;
    /// # let mem = Mem::new();
    /// # let mut file = PageFile::create(&mem, "db", 512).unwrap();
    /// # for _ in 0..3 {
    /// #     let n = freelist::allocate(&mut file).unwrap();
    /// #     file.write(n, &mut file.new_page()).unwrap();
    /// # }
    /// // A page file with pages 1 to 3, and a pool of 2 frames in front of it.
    /// let pool = BufPool::new(file, 2, Box::new(Clock::new(2)), Box::new(NoLog));
    /// let one = PageNo::new(1).unwrap();
    /// let two = PageNo::new(2).unwrap();
    /// let three = PageNo::new(3).unwrap();
    ///
    /// // Two threads, or two guards in one thread, can read a page at once.
    /// let a = pool.fetch(one).unwrap();
    /// let b = pool.fetch(one).unwrap();
    /// let c = pool.fetch(two).unwrap();
    ///
    /// // Both frames are pinned now, so page 3 has nowhere to go.
    /// assert!(matches!(pool.fetch(three), Err(Error::AllPinned { frames: 2 })));
    ///
    /// // Dropping the guards on page 1 unpins its frame, and page 3 takes it.
    /// drop((a, b));
    /// assert_eq!(pool.fetch(three).unwrap().number(), three);
    /// # drop(c);
    /// ```
    pub fn fetch(&self, n: PageNo) -> Result<PageRef<'_>> {
        loop {
            let (frame, claimed) = self.pin(n, true)?;
            // A frame we just claimed comes with its own exclusive latch. Trade
            // it for a shared one, like every other reader.
            drop(claimed);
            let latch = self.read_latch(frame);
            if self.holds(frame, n) {
                return Ok(PageRef {
                    pool: self,
                    frame,
                    page: n,
                    latch: Some(latch),
                });
            }
            drop(latch);
            self.unpin(frame);
        }
    }
}

/// A page fetched for reading with [`BufPool::fetch`]. It derefs to the [`Page`].
///
/// While it lives, it holds one pin on the page's frame, so the page isn't evicted,
/// and the frame's shared latch, so no thread can change the page. Dropping it
/// lets go of the latch, then the pin.
pub struct PageRef<'a> {
    /// The pool the page is in.
    pool: &'a BufPool,
    /// The frame that holds the page.
    frame: usize,
    /// The page's number.
    page: PageNo,
    /// The frame's shared latch. It's an `Option` only so that `drop` can let go
    /// of it before the pin. It's `Some` until then.
    latch: Option<RwLockReadGuard<'a, Page>>,
}

impl PageRef<'_> {
    /// The number of the page this guard holds.
    pub fn number(&self) -> PageNo {
        self.page
    }
}

impl Deref for PageRef<'_> {
    type Target = Page;

    fn deref(&self) -> &Self::Target {
        self.latch.as_ref().expect("the latch is held until drop")
    }
}

impl Drop for PageRef<'_> {
    fn drop(&mut self) {
        self.latch = None;
        self.pool.unpin(self.frame);
    }
}
