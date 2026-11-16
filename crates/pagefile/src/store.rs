//! `PageStore`: the trait for reading and writing pages and the header.
//!
//! `PageFile` implements it by reading and writing the file. The freelist and
//! overflow chains are functions that take any `PageStore`. So a cache in
//! front of the file can implement the trait, and they work through the cache
//! too, header and free pages included.
//!
//! ```text
//! freelist, overflow  --- PageStore --->  PageFile         (the file itself)
//!                                     or  a cache in front of a PageFile
//! ```

use crate::{Header, Page, PageNo, Result};

/// A place to read and write numbered pages, and the header that describes
/// them.
///
/// What callers can rely on: reading a page returns the bytes last written
/// to that page number, and reading the header returns the header last
/// written. Pages from 1 to the header's page count minus 1 exist. The
/// trait does not promise durability. Each implementer decides when its
/// writes reach the disk, with a sync of its own.
///
/// What implementers must do: keep those promises, and convert this crate's
/// errors into their own error type, so that the freelist and overflow
/// functions can return them.
///
/// It is a trait, and not only methods on `PageFile`, so that code which
/// changes pages, like the freelist, is written once and works on anything
/// that holds pages. A cache in front of the file can implement it. Then
/// every page the freelist reads or writes goes through the cache, and not
/// directly to the file.
///
/// # Examples
///
/// Using it through a `PageFile`:
///
/// ```
/// # use pagefile::{PageFile, PageNo, PageStore};
/// # use vfs::Mem;
/// # let mem = Mem::new();
/// # let mut file = PageFile::create(&mem, "db", 512).unwrap();
/// # let mut header = file.read_header().unwrap();
/// # header.page_count = 2;
/// # file.write_header(&header).unwrap();
/// // A page file whose header counts pages 0 and 1.
/// let one = PageNo::new(1).unwrap();
/// let mut page = file.new_page();
/// page.put_u32(100, 42);
/// file.write(one, &mut page).unwrap();
///
/// let mut back = file.new_page();
/// file.read(one, &mut back).unwrap();
/// assert_eq!(back.u32_at(100), 42);
///
/// // Page 2 is past the page count.
/// assert!(file.read(PageNo::new(2).unwrap(), &mut back).is_err());
/// ```
///
/// Implementing it, here for pages kept in memory:
///
/// ```
/// use pagefile::{Error, Header, Page, PageNo, PageStore};
///
/// struct MemStore {
///     header: Header,
///     /// Page `n` is `pages[n]`. `pages[0]` stands for the header and is unused.
///     pages: Vec<Vec<u8>>,
/// }
///
/// impl MemStore {
///     fn check(&self, n: PageNo) -> Result<(), Error> {
///         let count = self.header.page_count;
///         if n.get() >= count {
///             return Err(Error::OutOfRange { page: n.get(), count });
///         }
///         Ok(())
///     }
/// }
///
/// impl PageStore for MemStore {
///     type Error = Error;
///
///     fn page_size(&self) -> usize {
///         self.header.page_size as usize
///     }
///
///     fn read_header(&self) -> Result<Header, Error> {
///         Ok(self.header.clone())
///     }
///
///     fn write_header(&mut self, header: &Header) -> Result<(), Error> {
///         let size = header.page_size as usize;
///         self.pages.resize(header.page_count as usize, vec![0; size]);
///         self.header = header.clone();
///         Ok(())
///     }
///
///     fn read(&self, n: PageNo, page: &mut Page) -> Result<(), Error> {
///         self.check(n)?;
///         page.bytes_mut().copy_from_slice(&self.pages[n.get() as usize]);
///         page.verify(n.get())
///     }
///
///     fn write(&mut self, n: PageNo, page: &mut Page) -> Result<(), Error> {
///         self.check(n)?;
///         page.seal();
///         self.pages[n.get() as usize] = page.bytes().to_vec();
///         Ok(())
///     }
/// }
///
/// let mut header = Header::new(512);
/// header.page_count = 2;
/// let mut store = MemStore { header: Header::new(512), pages: vec![] };
/// store.write_header(&header).unwrap();
///
/// let one = PageNo::new(1).unwrap();
/// let mut page = store.new_page();
/// page.put_u32(100, 42);
/// store.write(one, &mut page).unwrap();
/// let mut back = store.new_page();
/// store.read(one, &mut back).unwrap();
/// assert_eq!(back.u32_at(100), 42);
/// ```
pub trait PageStore {
    /// The store's error. It must hold this crate's errors too, because the
    /// freelist and overflow functions return them through it.
    ///
    /// It is an associated type, not this crate's `Error`, because a store
    /// can fail in ways this crate does not know about. A cache, for example,
    /// can run out of room. With its own error type, those failures come back
    /// through the freelist as themselves, and the caller can match on them.
    /// `From<crate::Error>` is the one thing it must offer: a way to wrap the
    /// errors this crate makes.
    type Error: From<crate::Error>;

    /// The size of every page, in bytes.
    fn page_size(&self) -> usize;

    /// A copy of the header in page 0, as last written.
    ///
    /// # Errors
    ///
    /// Any error the store has reading it. `PageFile` keeps the header in
    /// memory, so it never fails.
    fn read_header(&self) -> Result<Header, Self::Error>;

    /// Replaces the header in page 0. A caller that adds pages raises
    /// `page_count` here before it writes the new pages.
    ///
    /// # Errors
    ///
    /// Any error the store has writing it.
    fn write_header(&mut self, header: &Header) -> Result<(), Self::Error>;

    /// Reads page `n` into `page` and checks its checksum. `page` must be
    /// the store's page size.
    ///
    /// # Errors
    ///
    /// [`Error::OutOfRange`](crate::Error::OutOfRange) if `n` is at or past
    /// the page count, [`Error::Checksum`](crate::Error::Checksum) if the
    /// page is damaged, and any error the store has reading it.
    fn read(&self, n: PageNo, page: &mut Page) -> Result<(), Self::Error>;

    /// Sets the checksum of `page`, then writes it as page `n`. `page` must
    /// be the store's page size. The checksum changes `page` itself, which
    /// is why it is `&mut`.
    ///
    /// # Errors
    ///
    /// [`Error::OutOfRange`](crate::Error::OutOfRange) if `n` is at or past
    /// the page count, and any error the store has writing it.
    fn write(&mut self, n: PageNo, page: &mut Page) -> Result<(), Self::Error>;

    /// Makes a zeroed page of the store's page size, ready to fill and
    /// write, or to read into.
    fn new_page(&self) -> Page {
        Page::new(self.page_size())
    }
}
