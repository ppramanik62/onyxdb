//! The header in page 0: what the file says about itself.
//!
//! ```text
//! offset  size  field
//!      0     8  magic: the ASCII bytes "pagefile"
//!      8     2  format version, 1
//!     10     4  page size, in bytes
//!     14     4  page count, page 0 included
//!     18     4  first page on the freelist, 0 if the freelist is empty
//!     22     4  number of pages on the freelist
//!     26    32  user bytes, for the code using this crate
//!     58     .  zeros
//! size-4     4  CRC-32C of everything before it
//! ```
//!
//! Every number is little-endian.
//!
//! The header is a full page, not a small block, so that every page in the
//! file has the same size and page `n` is always at `n × page size`. Most of
//! page 0 is zeros, which costs one page per file.
//!
//! The 32 user bytes let the code that uses this crate keep a few numbers of
//! its own, such as where its data starts, without a page of its own and
//! without this crate knowing what they mean.
//!
//! The page size is inside page 0, but reading page 0 needs the page size.
//! So a file is opened in two steps: read the first [`PREFIX_LEN`] bytes to
//! learn the page size, then read all of page 0 and check its checksum.

use crate::{
    Error, Page, PageNo, Result,
    page::{check_page_size, link_to_u32},
};

/// The magic string at the start of every page file.
pub const MAGIC: [u8; 8] = *b"pagefile";

/// The format version this code reads and writes.
pub const VERSION: u16 = 1;

/// How many bytes the header keeps for the code that uses this crate. This
/// crate never reads them.
pub const USER_LEN: usize = 32;

/// How many bytes at the start of page 0 are enough to learn the page size:
/// the magic, the version and the page size.
pub const PREFIX_LEN: usize = 14;

/// The header, decoded: what the file says about itself.
///
/// It is a plain struct, so changing it changes nothing on disk. A
/// `PageStore`'s `write_header` encodes it and writes it to page 0.
///
/// # Examples
///
/// ```
/// use pagefile::{Header, Page};
///
/// let mut header = Header::new(512);
/// header.page_count = 3;
/// let mut page = Page::new(512);
/// header.encode(&mut page);
/// assert_eq!(&page.bytes()[..8], b"pagefile");
/// assert_eq!(Header::decode(&page).unwrap(), header);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    /// 10: bytes per page.
    pub page_size: u32,
    /// 14: pages in the file, page 0 included.
    pub page_count: u32,
    /// 18: the first page on the freelist, or `None` if it is empty.
    pub free_head: Option<PageNo>,
    /// 22: how many pages are on the freelist.
    pub free_count: u32,
    /// 26: bytes this crate never reads, kept for the code that uses it, for
    /// example to remember the page its data starts at.
    pub user: [u8; USER_LEN],
}

/// Reads the page size from the first [`PREFIX_LEN`] bytes of a file, after
/// checking the magic string and the version.
///
/// # Examples
///
/// ```
/// use pagefile::Error;
/// use pagefile::header::{PREFIX_LEN, page_size_from_prefix};
///
/// let mut prefix = [0; PREFIX_LEN];
/// prefix[..8].copy_from_slice(b"pagefile");
/// prefix[8] = 1; // version 1
/// prefix[10..14].copy_from_slice(&4096u32.to_le_bytes());
/// assert_eq!(page_size_from_prefix(&prefix).unwrap(), 4096);
///
/// prefix[0] = b'x';
/// assert!(matches!(page_size_from_prefix(&prefix), Err(Error::NotAPageFile)));
/// ```
///
/// # Errors
///
/// - [`Error::NotAPageFile`] if the bytes do not start with [`MAGIC`].
/// - [`Error::UnsupportedVersion`] if the version is not [`VERSION`].
/// - [`Error::BadPageSize`] if the page size is not one a file can use.
pub fn page_size_from_prefix(prefix: &[u8; PREFIX_LEN]) -> Result<usize> {
    if prefix[0..8] != MAGIC {
        return Err(Error::NotAPageFile);
    }
    let version = u16::from_le_bytes([prefix[8], prefix[9]]);
    if version != VERSION {
        return Err(Error::UnsupportedVersion(version));
    }
    let size = u32::from_le_bytes([prefix[10], prefix[11], prefix[12], prefix[13]]);
    check_page_size(size)
}

impl Header {
    /// The header of a new file with pages of `page_size` bytes: a page
    /// count of 1 for page 0 itself, an empty freelist, and zeroed user
    /// bytes.
    pub fn new(page_size: usize) -> Header {
        Header {
            page_size: page_size as u32,
            page_count: 1,
            free_head: None,
            free_count: 0,
            user: [0; USER_LEN],
        }
    }

    /// Writes the header into `page`, erasing whatever was there. It does not
    /// set the checksum. Writing the page does that.
    pub fn encode(&self, page: &mut Page) {
        page.bytes_mut().fill(0);
        page.bytes_mut()[0..8].copy_from_slice(&MAGIC);
        page.put_u16(8, VERSION);
        page.put_u32(10, self.page_size);
        page.put_u32(14, self.page_count);
        page.put_u32(18, link_to_u32(self.free_head));
        page.put_u32(22, self.free_count);
        page.bytes_mut()[26..26 + USER_LEN].copy_from_slice(&self.user);
    }

    /// Reads the header from page 0. It does not check the checksum, so
    /// check it first with [`Page::verify`].
    ///
    /// The checksum is left to the caller so that a tool can still decode a
    /// damaged header and show what is in it. Code that opens a file for use
    /// checks the checksum first.
    ///
    /// # Errors
    ///
    /// The same as [`page_size_from_prefix`]. The other fields are not
    /// checked here. Opening a file checks the page count against the
    /// file's length.
    ///
    /// # Examples
    ///
    /// ```
    /// use pagefile::{Header, Page};
    ///
    /// let mut page = Page::new(512);
    /// Header::new(512).encode(&mut page);
    /// // The checksum, in the last 4 bytes, was never set. Decoding works anyway.
    /// assert_eq!(page.u32_at(508), 0);
    /// assert_eq!(Header::decode(&page).unwrap().page_count, 1);
    /// ```
    pub fn decode(page: &Page) -> Result<Header> {
        let mut prefix = [0; PREFIX_LEN];
        prefix.copy_from_slice(&page.bytes()[..PREFIX_LEN]);
        let page_size = page_size_from_prefix(&prefix)?;
        let mut user = [0; USER_LEN];
        user.copy_from_slice(&page.bytes()[26..26 + USER_LEN]);
        Ok(Header {
            page_size: page_size as u32,
            page_count: page.u32_at(14),
            free_head: PageNo::new(page.u32_at(18)),
            free_count: page.u32_at(22),
            user,
        })
    }
}
