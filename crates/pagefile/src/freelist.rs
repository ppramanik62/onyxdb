//! The freelist: freed pages, reused before the file grows.
//!
//! Free pages form a chain that starts at the header. Each free page holds
//! the number of the next one. `allocate` takes the first page, or adds a page
//! at the end of the file if the chain is empty. `free` puts a page at the
//! front. Both work through any `PageStore`, so with a cache in front of the
//! file, every page they change goes through the cache, the header included.
//!
//! ```text
//! header.free_head -> page 5 -> page 2 -> page 7 -> 0 (end)
//! header.free_count = 3
//! ```
//!
//! `free` puts a page at the front of the chain, and `allocate` takes the
//! page at the front. So each call costs one page write and one header
//! write, and neither has to follow the chain. It also means that
//! `allocate` returns the page that was freed most recently, which is the
//! page most likely to still be in memory.
//!
//! The chain is stored inside the free pages themselves, so it uses no extra
//! space: a free page holds nothing else. The cost is that finding every
//! free page means reading the pages one by one.
//!
//! The file never shrinks. A freed page stays in the file until `allocate`
//! returns it again. To shrink the file, this crate would have to move the
//! pages at the end of the file into the free pages, and update every link
//! that points to the moved pages. Only the code that stores data in the
//! pages knows where those links are, so this crate cannot do it.
//!
//! Each call writes a page and the header, one after the other, and
//! neither syncs. A crash between the two writes can leave a page that is in
//! the file but on no freelist, or a freelist whose first page is no longer
//! free. A store shared between threads needs a lock around each call,
//! because each call reads the header and writes it back.
//!
//! ```text
//! free page   0      1     9                13
//!             +------+-----+----------------+-----------------+----------+
//!             | kind | LSN | next free page | (zeros)         | checksum |
//!             +------+-----+----------------+-----------------+----------+
//! ```

use crate::{Error, Header, PageKind, PageNo, PageStore, Result, page::link_to_u32};

/// 9: in a free page, the number of the next free page, or 0 for the last one.
pub const FREE_NEXT: usize = 9;

/// Returns a page that is free to use, with kind `Empty`: the first page on
/// the freelist if there is one, or a new page at the end of the file.
///
/// The page is written as `Empty` before it is returned, and the header is
/// updated. Its LSN is kept from before it was freed.
///
/// The page is written as `Empty` so that it never looks free after it is
/// allocated. Without this write, a reused page would still say `Free`, with
/// an old link in it, until the caller wrote something there. A new page at
/// the end of the file gets the same write, so it has a valid checksum and
/// can be read immediately.
///
/// `S: PageStore + ?Sized` means `S` is any type that implements
/// `PageStore`. The `?Sized` lets `S` also be a trait object, so code that
/// holds a `&mut dyn PageStore<Error = E>` can call this too. Without it,
/// Rust accepts only types whose size is known when the code is compiled,
/// and a trait object is not one of them.
///
/// # Errors
///
/// - [`Error::WrongKind`] if the first page on the freelist is not a free
///   page. The freelist is damaged then.
/// - [`Error::TooManyPages`] if the freelist is empty and the file already
///   has the most pages a u32 can count.
/// - Any error the store returns reading or writing. If adding a page at
///   the end fails, the page count is put back.
///
/// # Examples
///
/// ```
/// use pagefile::{PageFile, freelist};
/// use vfs::Mem;
///
/// let mem = Mem::new();
/// let mut file = PageFile::create(&mem, "db", 512).unwrap();
/// let a = freelist::allocate(&mut file).unwrap();
/// let b = freelist::allocate(&mut file).unwrap();
/// assert_eq!((a.get(), b.get()), (1, 2));
///
/// // A freed page is reused before the file grows.
/// freelist::free(&mut file, a).unwrap();
/// assert_eq!(freelist::allocate(&mut file).unwrap(), a);
/// assert_eq!(file.page_count(), 3);
/// ```
pub fn allocate<S: PageStore + ?Sized>(store: &mut S) -> Result<PageNo, S::Error> {
    let header = store.read_header()?;
    match header.free_head {
        Some(head) => pop(store, header, head),
        None => append(store, header),
    }
}

/// Puts page `n` on the freelist. Do not use it again until `allocate`
/// returns it.
///
/// The page is rewritten as a `Free` page that links to the old first free
/// page, and the header is updated to start the freelist at `n`. Its LSN is
/// kept.
///
/// Freeing a page twice is refused because it would put the page on the
/// list twice. `allocate` would then hand it to two callers, and each would
/// overwrite the other's data. Checking the kind byte catches it with one
/// read.
///
/// # Errors
///
/// - [`Error::DoubleFree`] if page `n` is already a free page. Nothing
///   changes then.
/// - Any error the store returns reading or writing, such as
///   [`Error::OutOfRange`] for a page the file does not have.
///
/// # Examples
///
/// ```
/// use pagefile::{Error, PageFile, freelist};
/// use vfs::Mem;
///
/// let mem = Mem::new();
/// let mut file = PageFile::create(&mem, "db", 512).unwrap();
/// let n = freelist::allocate(&mut file).unwrap();
/// freelist::free(&mut file, n).unwrap();
/// assert!(matches!(freelist::free(&mut file, n), Err(Error::DoubleFree(1))));
/// ```
pub fn free<S: PageStore + ?Sized>(store: &mut S, n: PageNo) -> Result<(), S::Error> {
    let mut header = store.read_header()?;
    let mut page = store.new_page();
    store.read(n, &mut page)?;
    if page.kind() == Some(PageKind::Free) {
        return Err(Error::DoubleFree(n.get()).into());
    }
    page.reset(PageKind::Free);
    page.put_u32(FREE_NEXT, link_to_u32(header.free_head));
    store.write(n, &mut page)?;
    header.free_head = Some(n);
    header.free_count += 1;
    store.write_header(&header)
}

/// Takes `head`, the first page on the freelist, off the list and empties
/// it. `header` is the store's current header, and `head` its free head.
fn pop<S: PageStore + ?Sized>(
    store: &mut S,
    mut header: Header,
    head: PageNo,
) -> Result<PageNo, S::Error> {
    let mut page = store.new_page();
    store.read(head, &mut page)?;
    page.expect_kind(head, PageKind::Free)?;
    let next = PageNo::new(page.u32_at(FREE_NEXT));
    page.reset(PageKind::Empty);
    store.write(head, &mut page)?;
    header.free_head = next;
    // A count of 0 here is wrong. Leave it at 0 for `check` to report, instead
    // of wrapping around.
    header.free_count = header.free_count.saturating_sub(1);
    store.write_header(&header)?;
    Ok(head)
}

/// Adds an empty page at the end of the file. `header` is the store's
/// current header. The header counts the page first, because a store only
/// writes pages below the page count.
fn append<S: PageStore + ?Sized>(store: &mut S, mut header: Header) -> Result<PageNo, S::Error> {
    let n = header.page_count;
    header.page_count = n.checked_add(1).ok_or(Error::TooManyPages)?;
    store.write_header(&header)?;
    let n = PageNo::new(n).expect("page 0 always exists, so a new page is never page 0");
    let mut page = store.new_page();
    page.reset(PageKind::Empty);
    if let Err(e) = store.write(n, &mut page) {
        // Take the page back out of the count, so a failed allocate changes
        // nothing.
        header.page_count = n.get();
        store.write_header(&header)?;
        return Err(e);
    }
    Ok(n)
}
