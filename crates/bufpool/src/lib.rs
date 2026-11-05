#![doc = include_str!("../README.md")]
#![warn(missing_docs)]

mod error;
pub mod log;
pub mod lruk;
pub mod policy;
pub mod pool;
pub mod store;
pub mod table;

pub use error::{Error, Result};
pub use log::{LogHook, NoLog};
pub use lruk::LruK;
pub use policy::{Clock, Policy};
pub use pool::{BufPool, PageMut, PageRef, Stats};
pub use store::{HeaderMut, HeaderRef};
pub use table::PageTable;
