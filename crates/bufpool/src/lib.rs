#![doc = include_str!("../README.md")]
#![warn(missing_docs)]

pub mod log;
pub mod policy;
pub mod table;

pub use log::{LogHook, NoLog};
pub use policy::{Clock, Policy};
pub use table::PageTable;
