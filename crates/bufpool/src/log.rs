//! `LogHook`: how the pool keeps the write-ahead rule without knowing about the log.
//!
//! Each page stores an LSN: the log sequence number of the last log record that
//! changed it. The write-ahead rule says a page may reach the file only after
//! every log upto the page's LSN is durable. Otherwise a crash could leave a change
//! in the file with no log record to undo or redo it.
//!
//! The pool doesn't know where the log is or how it's written. Before it writes a
//! page, it calls [`LogHook::flush_to`] with that page's LSN. The code that owns
//! the log, implements the trait. A pool with no log uses [`NoLog`].
//!
//! A trait keeps the pool independent of how the log works. The pool only needs
//! one promise, "the log is durable up to here", and the code that owns the log
//! can keep it however it likes: one write per record, records grouped together,
//! or no log at all in a test.

use std::error;

/// What the pool calls before it writes a page, to keep the write-ahead rule.
///
/// The pool may call it from any thread, and from many threads at once, so it's
/// `Send` and `Sync`. It's never called while the pool holds its mutex, so it
/// may block on I/O.
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
    /// Makes every log record up to and including `lsn` durable, and returns once
    /// they are. Once this returns `Ok`, a page with that LSN may be written.
    ///
    /// The pool calls it before every page write, often with an LSN that's already
    /// durable, so that case should be cheap. Before writing the header, which has
    /// no LSN, the pool calls it with `u64::MAX`, which means the whole log.
    ///
    /// # Errors
    ///
    /// Any error that kept the log from being made durable. The pool then doesn't
    /// write the page, and returns the error as `Error::Log`.
    ///
    /// The error is a `Box<dyn Error>` because the pool doesn't know what kind of
    /// log is behind the hook, so it can't name its error type. It only passes
    /// the error on.
    fn flush_to(&self, lsn: u64) -> Result<(), Box<dyn error::Error + Send + Sync>>;
}

/// The hook for a pool with no log. `flush_to` always returns `Ok` at once,
/// so pages are written whenever the pool chooses.
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
