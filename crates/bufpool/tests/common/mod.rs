//! Helpers the tests share. Each test file uses only some of them.
#![allow(dead_code)]

use bufpool::{BufPool, Clock, NoLog};
use pagefile::page::BODY;
use pagefile::{PageFile, PageNo, PageStore, freelist};
use vfs::Mem;

/// Where a test page keeps its stamp, a u32. Page `n` starts with stamp `n`.
pub const STAMP: usize = BODY;

/// Page number `n`. Panics on 0.
pub fn no(n: u32) -> PageNo {
    PageNo::new(n).expect("page numbers start at 1")
}

/// Creates `test.pf` in `mem`, with 512-byte pages and `pages` pages after
/// the header, and syncs it. Page `n` holds the stamp `n`.
pub fn file_with(mem: &Mem, pages: u32) -> PageFile {
    let mut file = PageFile::create(mem, "test.pf", 512).unwrap();
    let mut page = file.new_page();
    for _ in 0..pages {
        let n = freelist::allocate(&mut file).unwrap();
        page.put_u32(STAMP, n.get());
        file.write(n, &mut page).unwrap();
    }
    file.sync().unwrap();
    file
}

/// A pool of `frames` frames, with CLOCK and no log, over a new `test.pf`
/// of `pages` pages.
pub fn pool(mem: &Mem, pages: u32, frames: usize) -> BufPool {
    let file = file_with(mem, pages);
    BufPool::new(file, frames, Box::new(Clock::new(frames)), Box::new(NoLog))
}
