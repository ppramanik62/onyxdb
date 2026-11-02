#![doc = include_str!("../README.md")]
#![warn(missing_docs)]

mod error;
pub mod log;
pub mod policy;
pub mod pool;
pub mod table;

pub use error::{Error, Result};
pub use log::{LogHook, NoLog};
pub use policy::{Clock, Policy};
pub use pool::{BufPool, PageRef, Stats};
pub use table::PageTable;
