#![doc = include_str!("../README.md")]
// Every public item must say what it is. The compiler checks this for us.
#![warn(missing_docs)]

pub mod check;

pub mod crc;

mod error;
pub mod file;

pub mod freelist;

pub mod header;

pub mod overflow;
pub mod page;

pub mod slotted;
pub mod store;

pub use error::{Error, Result};

pub use file::PageFile;

pub use header::Header;
pub use page::{Page, PageKind, PageNo};

pub use slotted::Slotted;
pub use store::PageStore;
