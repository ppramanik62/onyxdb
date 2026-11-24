//! The header, `flush`, and `PageStore` on top of the pool.
//!
//! The pool keeps its own copy of the header, and it never evicts it. New
//! pages and freelist changes are counted in this copy first, before the
//! header on disk. `flush` writes the dirty pages and then the header, so
//! when it returns, the file on disk is complete and consistent.
//!
//! The pool's copy of the header is separate from the file's for two
//! reasons. First, writing the header to the file on every allocation would
//! cost one write per allocation. Second, the header on disk must never
//! count a page that the file does not have yet. So the pool counts new
//! pages in memory, and `flush` writes the header last, after the pages it
//! counts are in the file.
//!
//! `PageStore` is implemented for `&BufPool`. So the freelist and overflow
//! functions from `pagefile` work through the pool. Every page they change,
//! and the header too, is changed in memory first.
//!
//! ```text
//! freelist::allocate(&mut &pool)
//!   -> read_header   the pool's copy of the header
//!   -> read, write   pages through the frames
//!   -> write_header  the pool's copy, marked dirty
//! pool.flush()       dirty pages, sync, header, sync
//! ```

use std::{
    ops::{Deref, DerefMut},
    sync::{RwLockReadGuard, RwLockWriteGuard},
};

use pagefile::{Header, Page, PageNo, PageStore};

use crate::{BufPool, Error, PageMut, Result};

/// The pool's header, for reading, from [`BufPool::header`]. It derefs to
/// the [`Header`].
///
/// It holds the header's shared latch, so other threads can read the header
/// but no thread can change it until this is dropped.
pub struct HeaderRef<'a>(RwLockReadGuard<'a, Header>);

/// The pool's header, for changing, from [`BufPool::header_mut`]. It derefs
/// to the [`Header`], mutably too.
///
/// It holds the header's exclusive latch, so no other thread can read or
/// change the header until this is dropped. The header was marked dirty
/// when this was made, so the next [`flush`](BufPool::flush) writes it.
pub struct HeaderMut<'a>(RwLockWriteGuard<'a, Header>);

impl BufPool {
    /// The pool's copy of the header, for reading. It can be ahead of the
    /// header in the file, until the next [`flush`](Self::flush).
    ///
    /// # Deadlocks
    ///
    /// Blocks while a [`HeaderMut`] lives. A thread that holds one and calls
    /// this waits forever.
    ///
    /// # Panics
    ///
    /// Panics if a thread panicked while it held the header's latch, because
    /// the header may then be half changed.
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
    /// let pool = BufPool::new(file, 2, Clock::new, Box::new(NoLog));
    /// // Page 0, the header, and pages 1 to 3.
    /// assert_eq!(pool.header().page_count, 4);
    ///
    /// // Allocating through the pool changes the pool's header.
    /// let n = pagefile::freelist::allocate(&mut &pool).unwrap();
    /// assert_eq!(n.get(), 4);
    /// assert_eq!(pool.header().page_count, 5);
    /// ```
    pub fn header(&self) -> HeaderRef<'_> {
        HeaderRef(
            self.header
                .read()
                .expect("a thread panicked while changing the header"),
        )
    }

    /// The pool's copy of the header, for changing. It is marked dirty at
    /// once, and the next [`flush`](Self::flush) writes it.
    ///
    /// # Deadlocks
    ///
    /// Blocks while any [`HeaderRef`] or other `HeaderMut` lives. A thread
    /// that holds one and calls this waits forever.
    ///
    /// # Panics
    ///
    /// Panics if a thread panicked while it held the header's latch or the
    /// pool's mutex, because the header or the pool's state may then be
    /// half changed.
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
    /// let pool = BufPool::new(file, 2, Clock::new, Box::new(NoLog));
    /// pool.header_mut().user[0] = 7;
    /// assert_eq!(pool.header().user[0], 7);
    /// ```
    pub fn header_mut(&self) -> HeaderMut<'_> {
        let latch = self
            .header
            .write()
            .expect("a thread panicked while changing the header");
        // Set while the exclusive latch is held, like a page's dirty flag.
        self.lock().header_dirty = true;
        HeaderMut(latch)
    }
}

impl Deref for HeaderRef<'_> {
    type Target = Header;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Deref for HeaderMut<'_> {
    type Target = Header;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for HeaderMut<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl BufPool {
    /// Makes the file on disk hold every change made through the pool so far.
    ///
    /// It writes every page that is dirty when it starts, and syncs the file.
    /// Then, if the header changed, it writes the header and syncs again.
    /// The pages are written first, so a crash during `flush` never leaves a
    /// header that counts pages the file does not have.
    ///
    /// Other threads can use the pool during `flush`, and pages stay in
    /// their frames. A page or header that another thread holds for changing
    /// is written after that thread drops its guard, so `flush` waits for
    /// those guards.
    ///
    /// # Errors
    ///
    /// [`Error::Log`] if the log cannot be made durable, and
    /// [`Error::PageFile`] if a write or a sync fails. Pages that were not
    /// written stay dirty, and calling `flush` again tries them again.
    ///
    /// # Deadlocks
    ///
    /// A thread that holds a [`PageMut`], [`HeaderRef`] or [`HeaderMut`]
    /// and calls `flush` may wait forever.
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
    /// let pool = BufPool::new(file, 2, Clock::new, Box::new(NoLog));
    /// let one = PageNo::new(1).unwrap();
    /// pool.fetch_mut(one).unwrap().set_lsn(9);
    /// pool.flush().unwrap();
    /// assert_eq!(pool.stats().writes, 1);
    ///
    /// // Nothing is dirty now, so a second flush writes nothing.
    /// pool.flush().unwrap();
    /// assert_eq!(pool.stats().writes, 1);
    /// ```
    pub fn flush(&self) -> Result<()> {
        // Copy the header first. Every page that this copy counts is already
        // in the file or dirty in a frame. So the dirty pages written next
        // include every page the copy counts.
        let copy = {
            let header = self.header();
            self.lock().header_dirty.then(|| header.clone())
        };
        self.write_dirty()?;
        self.file.sync()?;
        let Some(copy) = copy else {
            return Ok(());
        };
        // Page 0 has no LSN, so the whole log must be durable before the
        // header is written.
        self.log.flush_to(u64::MAX).map_err(Error::Log)?;
        let mut page = Page::new(self.page_size);
        copy.encode(&mut page);
        self.file.write_page(0, &mut page)?;
        self.file.sync()?;
        // The header is clean only if no thread changed it after the copy.
        let header = self.header();
        if *header == copy {
            self.lock().header_dirty = false;
        }
        Ok(())
    }
    /// Fetches page `n` for changing without reading it from the file. If
    /// the page is not in the pool, its frame starts as zeros. It is for a
    /// caller that replaces the whole page, such as `PageStore::write`, and
    /// it saves one read. Errors and deadlocks are the same as for
    /// `fetch_mut`.
    fn overwrite(&self, n: PageNo) -> Result<PageMut<'_>> {
        loop {
            let (frame, claimed) = self.pin(n, false)?;
            let latch = match claimed {
                Some(latch) => latch,
                None => self.write_latch(frame),
            };
            if self.mark_dirty(frame, n) {
                return Ok(PageMut {
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
    /// Checks that page `n` is below the page count in the pool's header.
    ///
    /// `PageStore` promises this check, and the freelist depends on it. The
    /// freelist raises the page count before it writes a new page, so a page
    /// at or past the count was never allocated. Reading such a page means a
    /// link is damaged. Writing one would make the file longer without the
    /// header counting the new page.
    ///
    /// `fetch` does not do this check. Most callers already know that the
    /// page exists, and the check would make every fetch take the header's
    /// latch.
    ///
    /// # Errors
    ///
    /// [`Error::PageFile`] holding [`pagefile::Error::OutOfRange`] if `n` is
    /// at or past the page count.
    fn check(&self, n: PageNo) -> Result<()> {
        let count = self.header().page_count;
        if n.get() >= count {
            let e = pagefile::Error::OutOfRange {
                page: n.get(),
                count,
            };
            return Err(e.into());
        }
        Ok(())
    }
}

/// The pool as a [`PageStore`], so the freelist and overflow functions in
/// `pagefile` work through the frames instead of the file.
///
/// The pool needs this because those functions change pages and the
/// header. If they wrote to the file directly, a page could change in the
/// file while an older copy of it was still dirty in a frame. The next
/// write-back would then write the old copy over the new one. A direct
/// write would also skip the log hook and break the write-ahead rule. With
/// `PageStore`, every page has one copy, in the pool, and every write to the
/// file goes through the hook.
///
/// It is implemented on `&BufPool`, not on `BufPool`, for this reason:
///
/// - `PageStore::write` takes `&mut self`.
/// - Threads share a pool through `&BufPool`, so no thread has a
///   `&mut BufPool`.
/// - With the impl on `&BufPool`, `&mut self` is a `&mut &BufPool`: a
///   mutable borrow of the thread's own reference. Any thread can make one.
///
/// That is why callers write `freelist::allocate(&mut &pool)`. It is safe,
/// because the pool's latches already stop two threads from changing a page
/// at the same time. The `&mut` is not needed for that.
///
/// How it differs from `PageFile`:
///
/// - `write` copies the page into a frame and marks it dirty. It does not
///   set the checksum. The page may change again before it reaches the
///   file, so the checksum is set once, at write-back, from the bytes that
///   are written.
/// - Each call takes a guard and drops it before it returns. So a sequence
///   of calls, like `freelist::allocate`, is not atomic. To make it atomic,
///   the pool would have to hold one lock across many calls, and every
///   thread would wait for that lock, even threads that never allocate. A
///   caller that allocates from many threads holds its own lock around the
///   sequence.
/// - The error type is the pool's own [`Error`]. So an
///   [`AllPinned`](Error::AllPinned) or a [`Log`](Error::Log) error from
///   inside the freelist reaches the caller unchanged.
impl PageStore for &BufPool {
    type Error = Error;

    fn page_size(&self) -> usize {
        self.page_size
    }

    fn read_header(&self) -> Result<Header> {
        Ok(self.header().clone())
    }

    fn write_header(&mut self, header: &Header) -> Result<()> {
        *self.header_mut() = header.clone();
        Ok(())
    }

    fn read(&self, n: PageNo, page: &mut Page) -> Result<()> {
        self.check(n)?;
        let frame = self.fetch(n)?;
        page.bytes_mut().copy_from_slice(frame.bytes());
        Ok(())
    }

    fn write(&mut self, n: PageNo, page: &mut Page) -> Result<()> {
        self.check(n)?;
        let mut frame = self.overwrite(n)?;
        frame.bytes_mut().copy_from_slice(page.bytes());
        Ok(())
    }
}
