//! One ordered key-value interface for every storage engine, a model engine
//! that is simple enough to trust, and a test suite that holds any engine to
//! the same promises.
//!
//! The code above storage needs four things from an engine: read a key, scan
//! a range of keys in order, write a group of changes all together, and know
//! when those changes survive a crash. [`Kv`] is the trait with exactly those
//! operations. Every engine implements it, so the code above can run on any
//! of them, and one test suite can check all of them.
//!
//! | Term | Meaning |
//! |---|---|
//! | key, value | Byte strings. Keys are ordered by comparing their bytes. |
//! | batch | A list of puts and deletes that an engine applies whole or not at all. |
//! | engine | A type that implements [`Kv`]. |
//! | applied | `apply` returned `Ok`. Every later read sees the batch. |
//! | synced | Applied with `sync` set. The batch survives a crash. |
//! | model | [`Model`], the engine that the suite trusts least to be wrong. |
//!
//! ```text
//!         get · scan(from, to) · apply(batch, sync)
//!                        │
//!   ┌────────────────────▼──────────────────────┐
//!   │ trait Kv                                  │
//!   └───────┬──────────────────────┬────────────┘
//!         Model                 other engines
//!   BTreeMap + a wal of batches
//!
//!   suite: random steps on an engine and on a BTreeMap, compared;
//!          a failure shrunk to a few steps; a crash at every write
//! ```
//!
//! - `error`: the errors every engine returns.
//! - `batch`: [`Batch`], the only way to write, and its encoding in bytes.
//! - `kv`: the [`Kv`] trait and the promises an engine makes.
//! - `model`: [`Model`], the model engine.
//! - `suite`: the tests that any engine can run.

// Every public item must say what it is. The compiler checks this for us.
#![warn(missing_docs)]

/// The longest key, in bytes, that a batch accepts.
///
/// An engine that keeps keys inside fixed-size pages must fit several of the
/// longest keys in one page, so that a full page can always be split in
/// two. 512 bytes leaves room for seven of them in a 4 KiB page.
pub const MAX_KEY: usize = 512;

/// The longest value, in bytes, that a batch accepts: 1 MiB.
///
/// A batch is held in memory whole while an engine applies it, and an
/// engine may write it to its log as one record. The limit keeps the memory
/// that one batch needs small and known.
pub const MAX_VALUE: usize = 1 << 20;

mod error;

pub use error::{Error, Result};
