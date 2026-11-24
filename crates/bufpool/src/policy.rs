//! Eviction policies: which frame to empty when the pool needs room.
//!
//! The pool owns the frames and the pins. A policy only remembers how pages
//! were used and picks a victim when asked. The pool talks to it in three
//! calls:
//!
//! ```text
//! fetch finds or loads page p in frame f   ->  touch(f, p)
//! pool needs an empty frame                ->  victim(evictable)
//! pool empties frame f                     ->  forget(f)
//! ```
//!
//! The pool decides which frames can be evicted: a frame with no pins that
//! holds a page. The policy decides which of those goes.
//!
//! This split keeps the pins in one place. If the policy tracked pins too,
//! the pool and the policy would each have a copy, and a bug in either one
//! could evict a page that a thread is using. Because the pool passes in
//! `evictable`, the policy cannot pick a pinned frame, even if everything
//! else it knows is wrong.
//!
//! This module has the [`Policy`] trait and [`Clock`]. `LruK` is in a
//! module of its own.

use pagefile::PageNo;

/// Picks which frame to empty when the pool needs one.
///
/// Frames are numbered from 0 to the pool's frame count minus 1, and a
/// policy is made for a fixed number of frames. The pool calls every method
/// while it holds its mutex, so no two calls run at the same time. Every call
/// should be fast: no I/O and no locks.
///
/// What the pool promises an implementer:
///
/// - The pool calls `touch(f, p)` on every fetch of page `p`, hit or miss,
///   after the page is in frame `f`.
/// - The pool calls `forget(f)` when frame `f`'s page leaves the pool. It
///   always calls it before the first `touch(f, _)` for the next page.
/// - The pool calls `victim` only when no frame is empty.
///
/// # Examples
///
/// The simplest policy that works: evict the lowest-numbered frame the pool
/// allows. It ignores how pages were used, so it is only good for tests.
///
/// ```
/// use bufpool::Policy;
/// use pagefile::PageNo;
///
/// struct Lowest {
///     frames: usize,
/// }
///
/// impl Policy for Lowest {
///     fn touch(&mut self, _frame: usize, _page: PageNo) {}
///
///     fn victim(&mut self, evictable: &dyn Fn(usize) -> bool) -> Option<usize> {
///         (0..self.frames).find(|&f| evictable(f))
///     }
///
///     fn forget(&mut self, _frame: usize) {}
/// }
///
/// let mut policy = Lowest { frames: 4 };
/// assert_eq!(policy.victim(&|f| f >= 2), Some(2));
/// ```
pub trait Policy: Send {
    /// Records that page `page` was just fetched, and that it is in `frame`.
    /// A policy that tracks recency or frequency updates it here.
    fn touch(&mut self, frame: usize, page: PageNo);

    /// Picks a frame to empty among the frames where `evictable` returns
    /// true, or returns `None` if there is no such frame.
    ///
    /// The returned frame must be one `evictable` accepted. Its page is still
    /// in the pool when this returns. The pool calls [`forget`](Self::forget)
    /// once the frame is emptied, so `victim` should not forget it itself.
    fn victim(&mut self, evictable: &dyn Fn(usize) -> bool) -> Option<usize>;

    /// Records that `frame` was emptied and its page is no longer in the
    /// pool. The next `touch` of this frame is for a different page.
    fn forget(&mut self, frame: usize);
}

/// A boxed policy is a policy, so code that chooses its policy while it
/// runs can hand the pool a `Box<dyn Policy>`.
impl<P: Policy + ?Sized> Policy for Box<P> {
    fn touch(&mut self, frame: usize, page: PageNo) {
        (**self).touch(frame, page);
    }

    fn victim(&mut self, evictable: &dyn Fn(usize) -> bool) -> Option<usize> {
        (**self).victim(evictable)
    }

    fn forget(&mut self, frame: usize) {
        (**self).forget(frame);
    }
}

/// The highest a frame's usage count goes. PostgreSQL uses the same cap,
/// `BM_MAX_USAGE_COUNT` in its `buf_internals.h`. A cap of 1 makes this the
/// classic CLOCK with one reference bit per frame.
pub const MAX_USAGE: u8 = 5;

/// CLOCK, with a usage count per frame, as PostgreSQL's clock sweep does it.
///
/// Every fetch adds 1 to the frame's count, up to [`MAX_USAGE`]. To find a
/// victim, a hand goes around the frames. It skips frames that cannot be
/// evicted, takes 1 off each count it passes, and stops at the first count
/// that is already 0. So a page fetched often survives several trips of the
/// hand, and a page fetched once is evicted on the next trip.
///
/// It uses one byte per frame, and a victim takes at most
/// `(MAX_USAGE + 1) * frames` steps to find.
///
/// It is the default policy because its work on a fetch is to add 1 to one
/// byte. Every fetch, hit or miss, runs this work under the pool's mutex.
/// True LRU would move the page to the front of a list on every fetch.
/// CLOCK does its real work when it looks for a victim, which happens only
/// on a miss. The cost is that it only approximates "least recently used".
///
/// # Examples
///
/// ```
/// use bufpool::{Clock, Policy};
/// use pagefile::PageNo;
///
/// let mut clock = Clock::new(2);
/// clock.touch(0, PageNo::new(5).unwrap());
/// clock.touch(1, PageNo::new(6).unwrap());
/// clock.touch(1, PageNo::new(6).unwrap());
/// // Frame 0 was fetched once and frame 1 twice, so frame 0 goes first.
/// assert_eq!(clock.victim(&|_| true), Some(0));
/// // With no evictable frame, there's no victim.
/// assert_eq!(clock.victim(&|_| false), None);
/// ```
pub struct Clock {
    /// The usage count of each frame.
    usage: Vec<u8>,
    /// The frame the hand looks at next.
    hand: usize,
}

impl Clock {
    /// Makes a clock for a pool of `frames` frames, with every count at 0
    /// and the hand at frame 0.
    pub fn new(frames: usize) -> Clock {
        Clock {
            usage: vec![0; frames],
            hand: 0,
        }
    }
}

impl Policy for Clock {
    fn touch(&mut self, frame: usize, _page: PageNo) {
        self.usage[frame] = (self.usage[frame] + 1).min(MAX_USAGE)
    }

    fn victim(&mut self, evictable: &dyn Fn(usize) -> bool) -> Option<usize> {
        // An evictable frame reaches 0 after at most MAX_USAGE visits and is
        // taken on the next one, so MAX_USAGE + 1 trips around are enough.
        // If no frame is evictable, the loop ends and there is no victim.
        for _ in 0..(MAX_USAGE as usize + 1) * self.usage.len() {
            let frame = self.hand;
            self.hand = (self.hand + 1) % self.usage.len();
            if !evictable(frame) {
                continue;
            }
            if self.usage[frame] == 0 {
                return Some(frame);
            }
            self.usage[frame] -= 1;
        }
        None
    }

    fn forget(&mut self, frame: usize) {
        self.usage[frame] = 0;
    }
}
