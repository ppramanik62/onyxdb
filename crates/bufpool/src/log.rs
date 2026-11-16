//! `LogHook`: how the pool keeps the write-ahead rule without knowing about
//! the log.
//!
//! Each page stores an LSN: the log sequence number of the last log record
//! that changed it. The write-ahead rule says a page may reach the file only
//! after every log record up to the page's LSN is durable. Otherwise a crash
//! could leave a change in the file with no log record to undo or redo it.
//!
//! The pool does not know where the log is or how it is written. Before it
//! writes a page, it calls [`LogHook::flush_to`] with that page's LSN. The
//! code that owns the log implements the trait. A pool with no log uses
//! [`NoLog`].
//!
//! The hook is a trait so that the pool does not depend on how the log
//! works. The pool needs only one promise: "the log is durable up to this
//! LSN". The code that owns the log can keep that promise in any way: one
//! write per record, many records written together, or no log at all in a
//! test.

use std::error;

/// What the pool calls before it writes a page, to keep the write-ahead rule.
///
/// The pool may call it from any thread, and from many threads at the same
/// time, so it is `Send` and `Sync`. The pool never calls it while it holds
/// its mutex, so the hook may wait for I/O.
///
/// # Examples
///
/// A hook that records how far the log had to be flushed, as a test might:
///
/// ```
/// use std::sync::atomic::{AtomicU64, Ordering};
/// use bufpool::LogHook;
///
/// struct Highest(AtomicU64);
///
/// impl LogHook for Highest {
///     fn flush_to(&self, lsn: u64) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
///         // A real log would write and sync its records up to `lsn` here.
///         self.0.fetch_max(lsn, Ordering::Relaxed);
///         Ok(())
///     }
/// }
///
/// let hook = Highest(AtomicU64::new(0));
/// hook.flush_to(7).unwrap();
/// hook.flush_to(3).unwrap();
/// assert_eq!(hook.0.load(Ordering::Relaxed), 7);
/// ```
pub trait LogHook: Send + Sync {
    /// Makes every log record up to and including `lsn` durable, and returns
    /// once they are. Once this returns `Ok`, a page with that LSN may be
    /// written.
    ///
    /// The pool calls it before every page write, often with an LSN that is
    /// already durable, so that case should be fast. The header has no LSN.
    /// So before the pool writes the header, it calls this with `u64::MAX`,
    /// which means the whole log.
    ///
    /// # Errors
    ///
    /// Any error that kept the log from being made durable. The pool then
    /// does not write the page, and returns the error as `Error::Log`.
    ///
    /// The error is a `Box<dyn Error>` because the pool does not know what
    /// kind of log is behind the hook, so it cannot name its error type. It
    /// only passes the error on.
    fn flush_to(&self, lsn: u64) -> Result<(), Box<dyn error::Error + Send + Sync>>;
}

/// The hook for a pool with no log. `flush_to` always returns `Ok`
/// immediately, so the pool writes pages whenever it chooses.
///
/// # Examples
///
/// ```
/// use bufpool::{LogHook, NoLog};
///
/// assert!(NoLog.flush_to(42).is_ok());
/// ```
pub struct NoLog;

impl LogHook for NoLog {
    fn flush_to(&self, _lsn: u64) -> Result<(), Box<dyn error::Error + Send + Sync>> {
        Ok(())
    }
}
