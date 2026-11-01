#![doc = include_str!("../README.md")]
#![warn(missing_docs)]

pub mod policy;
pub mod table;

pub use policy::{Clock, Policy};
pub use table::PageTable;
