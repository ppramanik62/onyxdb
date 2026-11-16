//! Overflow chains: values too big for one page.
//!
//! A long value is cut into pieces, one per overflow page, and each page holds
//! the number of the next.
//!
//! ```text
//! 0      1     9                 13           15
//! +------+-----+-----------------+------------+------------------+----------+
//! | kind | LSN | next page       | bytes used | data             | checksum |
//! +------+-----+-----------------+------------+------------------+----------+
//! ```
//!
//! The chain does not store the value's total length. The code that points
//! to the first page, the owner, stores the length. Every function here
//! takes any `PageStore`.
//!
//! The owner must store the first page number somewhere anyway, and the
//! length fits next to it. A second copy of the length in the chain could
//! disagree with the first one. Each page does store how many bytes it
//! holds, so a reader can check every page against the length it was given.
//!
//! The functions take `S: PageStore + ?Sized`. The `?Sized` lets `S` be a
//! trait object too, such as `dyn PageStore<Error = E>`, as well as a plain
//! type like `PageFile`.
//!
//! ```text
//! a 1000-byte value with 512-byte pages, 493 data bytes per page:
//!
//! page 1: next 2, used 493 -> page 2: next 3, used 493 -> page 3: next 0, used 14
//! ```
//!
//! Pages come from the freelist and go back to it when the chain is freed.
//! Reading checks the kind and the length of every page against the total
//! length. So a damaged chain returns an error, even a chain that loops
//! back on itself, and the read never runs forever.

use crate::{
    Error, PageKind, PageNo, PageStore, Result, freelist,
    page::{CHECKSUM_LEN, link_to_u32},
};

/// 9: the next page in the chain, or 0 on the last page.
pub const NEXT: usize = 9;
/// 13: how many data bytes this page holds.
pub const USED: usize = 13;
/// 15: where the data starts.
pub const DATA: usize = 15;

/// How many data bytes one overflow page holds: the page minus the header
/// fields and the checksum. A value of `len` bytes takes
/// `len.div_ceil(capacity(page_size))` pages.
///
/// # Examples
///
/// ```
/// use pagefile::overflow::capacity;
///
/// assert_eq!(capacity(512), 493);
/// ```
pub fn capacity(page_size: usize) -> usize {
    page_size - CHECKSUM_LEN - DATA
}

/// Stores `data` in a new chain of overflow pages and returns the first page.
/// The caller keeps that page number and `data.len()`, because
/// [`read_chain`] needs both.
///
/// It allocates every page first, then writes them in order. It does not
/// sync.
///
/// The pages are allocated first because each page holds the number of the
/// next page, and that number must be known when the page is written. When
/// every page number is known before the first write, each page is written
/// only once.
///
/// # Errors
///
/// Any error from [`freelist::allocate`] or from writing a page. The pages
/// already allocated stay allocated then, and nothing points to them.
///
/// # Panics
///
/// Panics if `data` is empty: a chain holds at least one byte.
///
/// # Examples
///
/// ```
/// use pagefile::overflow::{free_chain, read_chain, write_chain};
/// use pagefile::{PageFile, PageStore};
/// use vfs::Mem;
///
/// let mem = Mem::new();
/// let mut file = PageFile::create(&mem, "db", 512).unwrap();
/// let value = vec![9; 1000];
/// let first = write_chain(&mut file, &value).unwrap();
/// assert_eq!(file.page_count(), 4);
/// assert_eq!(read_chain(&file, first, value.len()).unwrap(), value);
///
/// free_chain(&mut file, first).unwrap();
/// assert_eq!(file.read_header().unwrap().free_count, 3);
/// ```
pub fn write_chain<S: PageStore + ?Sized>(store: &mut S, data: &[u8]) -> Result<PageNo, S::Error> {
    assert!(
        !data.is_empty(),
        "an overflow chain needs at least one byte"
    );
    let pieces: Vec<&[u8]> = data.chunks(capacity(store.page_size())).collect();
    let mut pages = Vec::with_capacity(pieces.len());
    for _ in &pieces {
        pages.push(freelist::allocate(store)?);
    }
    let mut page = store.new_page();
    for (i, piece) in pieces.iter().enumerate() {
        page.reset(PageKind::Overflow);
        page.put_u32(NEXT, link_to_u32(pages.get(i + 1).copied()));
        page.put_u16(USED, piece.len() as u16);
        page.bytes_mut()[DATA..DATA + piece.len()].copy_from_slice(piece);
        store.write(pages[i], &mut page)?;
    }
    Ok(pages[0])
}

/// Reads the `len` bytes stored in the chain that starts at `first`.
/// `len` must be the length the chain was written with.
///
/// # Errors
///
/// - [`Error::BadChain`] if the chain ends before `len` bytes, continues
///   after them, or has a page whose used length is 0, too big for a page,
///   or more than the bytes left. A loop is caught this way too.
/// - [`Error::WrongKind`] if a page in the chain is not an overflow page.
/// - Any error the store returns reading a page.
///
/// # Examples
///
/// ```
/// use pagefile::Error;
/// use pagefile::overflow::{read_chain, write_chain};
/// use pagefile::PageFile;
/// use vfs::Mem;
///
/// let mem = Mem::new();
/// let mut file = PageFile::create(&mem, "db", 512).unwrap();
/// let first = write_chain(&mut file, &[1; 600]).unwrap();
///
/// // Asking for the wrong length is caught, not read as garbage.
/// assert!(matches!(read_chain(&file, first, 2000), Err(Error::BadChain(_))));
/// assert!(matches!(read_chain(&file, first, 100), Err(Error::BadChain(_))));
/// ```
pub fn read_chain<S: PageStore + ?Sized>(
    store: &S,
    first: PageNo,
    len: usize,
) -> Result<Vec<u8>, S::Error> {
    let cap = capacity(store.page_size());
    let mut data = Vec::with_capacity(len);
    let mut page = store.new_page();
    let mut next = Some(first);
    while data.len() < len {
        let Some(n) = next else {
            let why = format!(
                "the chain from page {first} ends after {} of {len} bytes",
                data.len()
            );
            return Err(Error::BadChain(why).into());
        };
        store.read(n, &mut page)?;
        page.expect_kind(n, PageKind::Overflow)?;
        // Every page must hold at least one byte, so a chain that loops back
        // on itself still ends after at most `len` pages.
        let used = page.u16_at(USED) as usize;
        let left = len - data.len();
        if used == 0 || used > cap || used > left {
            let why = format!(
                "page {n} says it holds {used} bytes, but it can hold 1 to {cap} and {left} are left"
            );
            return Err(Error::BadChain(why).into());
        }
        data.extend_from_slice(&page.bytes()[DATA..DATA + used]);
        next = PageNo::new(page.u32_at(NEXT));
    }
    if let Some(n) = next {
        let why =
            format!("the chain from page {first} holds all {len} bytes but goes on to page {n}");
        return Err(Error::BadChain(why).into());
    }
    Ok(data)
}

/// Frees every page of the chain that starts at `first`, from the first to
/// the last.
///
/// # Errors
///
/// - [`Error::WrongKind`] if a page in the chain is not an overflow page,
///   which also catches a chain that loops back to a page just freed.
/// - Any error from reading a page or from [`freelist::free`].
///
/// The pages before the one that failed are already freed then, and the
/// rest are still allocated.
///
/// # Examples
///
/// ```
/// use pagefile::overflow::{free_chain, write_chain};
/// use pagefile::{PageFile, PageStore};
/// use vfs::Mem;
///
/// let mem = Mem::new();
/// let mut file = PageFile::create(&mem, "db", 512).unwrap();
/// let first = write_chain(&mut file, &[1; 600]).unwrap();
/// free_chain(&mut file, first).unwrap();
/// assert_eq!(file.read_header().unwrap().free_count, 2);
///
/// // The freed pages are reused before the file grows.
/// write_chain(&mut file, &[2; 600]).unwrap();
/// assert_eq!(file.page_count(), 3);
/// ```
pub fn free_chain<S: PageStore + ?Sized>(store: &mut S, first: PageNo) -> Result<(), S::Error> {
    let mut page = store.new_page();
    let mut next = Some(first);
    while let Some(n) = next {
        store.read(n, &mut page)?;
        // If the chain loops, it comes back to a page this loop already freed.
        // That page's kind is now free, so the loop stops here.
        page.expect_kind(n, PageKind::Overflow)?;
        next = PageNo::new(page.u32_at(NEXT));
        freelist::free(store, n)?;
    }
    Ok(())
}
