//! `LruK`: evicts the page whose K-th most recent fetch is oldest.
//!
//! | Term | Meaning |
//! |---|---|
//! | fetch number | A counter that goes up by 1 on every fetch. It's the clock LRU-K uses. |
//! | K-th time | The fetch number of a page's K-th most recent fetch. |
//! | history | The last K fetch numbers of a page, kept even for a while after it leaves the pool. |
//!
//! Plain LRU looks only at the last fetch, so a page fetched once by a scan
//! looks as valuable as a page fetched a thousand times. LRU-K remembers the
//! times of each page's last K fetches, counted in fetches, not seconds. The
//! victim is the page whose K-th most recent fetch is farthest back. A page
//! with fewer than K fetches counts as infinitely far back, so a page seen
//! once goes before any page seen K times. Among pages with fewer than K
//! fetches, the one fetched least recently goes first.
//!
//! ```text
//! page   fetched at (newest first)   K = 2: 2nd newest
//! 7      40  31                      31
//! 3      39                          never: evicted first
//! 9      38  12                      12: evicted next
//! ```
//!
//! A page's history outlives its stay in the pool. If page 3 is evicted
//! and fetched again soon, its old fetch counts, so it's treated as a page
//! seen twice. The history keeps up to [`REMEMBER`] pages per frame that
//! have left the pool, and forgets the oldest half when it fills up.

use pagefile::PageNo;

use crate::{PageTable, Policy};

/// The largest K this implementation keeps times for. Each page's history
/// is a fixed array of this many fetch numbers, so a bigger K costs memory
/// for every page in the history.
pub const MAX_K: usize = 4;

/// How many pages that left the pool are remembered for each frame. The
/// LRU-K paper calls this the retained information period and measures it
/// in seconds. We measure it in pages, so the history has a fixed size and
/// fits in a table made once.
///
/// It has to be more than 0, or a page evicted and fetched again soon would
/// look brand new, and a scan could push out every page again. 8 per frame
/// is enough to remember pages across a short gap without letting the
/// history grow large.
pub const REMEMBER: usize = 8;

/// What LRU-K remembers about one page, whether it's in the pool or left
/// it recently.
#[derive(Clone, Copy)]
struct History {
    /// The fetch numbers of the page's last fetches, newest first. 0 means
    /// no fetch, because fetch numbers start at 1.
    times: [u64; MAX_K],
    /// True while the page is in a frame.
    cached: bool,
}

/// The LRU-K policy, from O'Neil, O'Neil and Weikum (SIGMOD '93).
///
/// It evicts the page whose K-th most recent fetch is furthest back. With
/// K = 2, a page fetched once by a scan goes before any page fetched twice,
/// so a scan of many pages can't push out the pages used over and over.
///
/// Finding a victim looks at every frame, so it takes time in proportion to
/// the number of frames. The history holds up to `frames * (1 + REMEMBER)`
/// pages.
///
/// Looking at every frame is the simple choice. Keeping the frames sorted by
/// their K-th time would make finding a victim quick, but every fetch would
/// then have to re-sort, and fetches are far more common than misses.
///
/// # Examples
///
/// ```
/// use bufpool::{LruK, Policy};
/// use pagefile::PageNo;
///
/// let page = |n| PageNo::new(n).unwrap();
/// let mut lru2 = LruK::new(2, 2);
/// // Page 1 in frame 0 is fetched twice. Then a scan fetches page 2 once.
/// lru2.touch(0, page(1));
/// lru2.touch(0, page(1));
/// lru2.touch(1, page(2));
/// // Page 2 was fetched last, but only once, so its frame goes first.
/// assert_eq!(lru2.victim(&|_| true), Some(1));
/// ```
pub struct LruK {
    /// How many fetches back to look.
    k: usize,
    /// The number of the last fetch. It goes up by 1 with every `touch`.
    now: u64,
    /// The page in each frame.
    frames: Vec<Option<PageNo>>,
    /// The history of pages in the pool and of pages that left it recently.
    history: PageTable<History>,
}

impl LruK {
    /// Makes LRU-K for a pool of `frames` frames, with an empty history.
    /// LRU-1 is plain LRU, and LRU-2 is the usual choice.
    ///
    /// # Panics
    ///
    /// Panics if `k` is 0 or more than [`MAX_K`].
    pub fn new(frames: usize, k: usize) -> LruK {
        assert!((1..=MAX_K).contains(&k), "k must be from 1 to {MAX_K}");
        LruK {
            k,
            now: 0,
            frames: vec![None; frames],
            history: PageTable::new(frames * (1 + REMEMBER)),
        }
    }

    /// Forgets the older half of the pages that aren't in the pool, to make
    /// room in the history. Older means a longer time since the last fetch.
    /// Pages in the pool are always kept.
    ///
    /// It drops half at once, not one page, because it has to sort the
    /// history to find the oldest. Dropping half means the next sort is many
    /// new pages away, so the cost of sorting is spread over all of them.
    fn trim(&mut self) {
        let mut gone: Vec<(u64, PageNo)> = self
            .history
            .iter()
            .filter(|(_, h)| !h.cached) // only pages that left the pool
            .map(|(page, h)| (h.times[0], page)) // pair each with its newest fetch time
            .collect();
        gone.sort_unstable();
        for &(_, page) in &gone[..gone.len().div_ceil(2)] {
            self.history.remove(page);
        }
    }
}

impl Policy for LruK {
    fn touch(&mut self, frame: usize, page: PageNo) {
        self.now += 1;
        let mut history = match self.history.get(page) {
            Some(history) => history,
            None => {
                if self.history.len() == self.history.capacity() {
                    self.trim();
                }
                History {
                    times: [0; MAX_K],
                    cached: true,
                }
            }
        };
        history.times.copy_within(..MAX_K - 1, 1);
        history.times[0] = self.now;
        history.cached = true;
        self.history.insert(page, history);
        self.frames[frame] = Some(page);
    }

    fn victim(&mut self, evictable: &dyn Fn(usize) -> bool) -> Option<usize> {
        // The smallest K-th time is the farthest back. A page with fewer than
        // K fetches has 0 there, so it comes first, and among those pages the
        // one fetched least recently goes first.
        (0..self.frames.len())
            .filter(|&f| evictable(f))
            .filter_map(|f| {
                let history = self.history.get(self.frames[f]?)?;
                Some(((history.times[self.k - 1], history.times[0]), f))
            })
            .min()
            .map(|(_, frame)| frame)
    }

    fn forget(&mut self, frame: usize) {
        let Some(page) = self.frames[frame].take() else {
            return;
        };
        if let Some(mut history) = self.history.get(page) {
            history.cached = false;
            self.history.insert(page, history);
        }
    }
}
