#![doc = include_str!("../README.md")]
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

mod batch;
mod error;
mod kv;
mod model;

pub use batch::{Batch, Op};
pub use error::{Error, Result};
pub use kv::{Iter, Kv, empty_range};
pub use model::Model;

pub mod suite;
