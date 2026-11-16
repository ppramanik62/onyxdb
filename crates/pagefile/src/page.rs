//! Pages, page numbers and page kinds.
//!
//! A page file is cut into pages of one size. Page 0 holds the header (see
//! `header`). Every other page starts with a kind byte that says what the page
//! holds, then an 8-byte log sequence number (LSN) for the code above this
//! crate. Every page, page 0 included, ends with a 4-byte checksum (see `crc`).
//!
//! A [`Page`] is just the bytes, in memory. It reads and writes numbers at
//! byte offsets, little-endian, and it does not know its own page number.
//! Each kind's module lays out the bytes between the LSN and the checksum.
//!
//! Every page stores its kind in its first byte. So code can tell what a
//! page holds from the page alone, without knowing which page links to it.
//! A tool that checks the whole file uses this. So does every function here
//! that checks that a page has the kind it expects.
//!
//! The LSN is in every page, at the same offset, even though this crate never
//! reads it. Code that keeps a log of changes needs to know, for each page,
//! the last logged change that the page contains. Because the field is
//! defined here, for every page, no page format can leave it out.
//!
//! Numbers are little-endian because that is the byte order of x86 and ARM,
//! so the bytes on disk are the bytes in memory. Any fixed order would work,
//! as long as it never changes.
//!
//! The checksum is at the end of the page. One CRC then covers every byte
//! before it, and every page layout starts at offset 0 with the kind byte.
//! If the checksum were at the start, every layout would have to begin after
//! it.
//!
//! ```text
//! byte 0      1          9                             size - 4    size
//!      +------+----------+-----------------------------+-----------+
//!      | kind | LSN      | laid out by the page's kind | CRC-32C   |
//!      +------+----------+-----------------------------+-----------+
//! ```

use std::{fmt, num::NonZeroU32};

use crate::{Error, Result, crc::crc32c};

/// The smallest page size a file can use, in bytes. It is the size of one
/// disk sector, the smallest unit a disk writes, so a page is never smaller
/// than a sector.
pub const MIN_PAGE_SIZE: usize = 512;

/// The largest page size, in bytes. Offsets inside a page are stored as u16,
/// which cannot hold 65536, so the largest power of two that fits is 32768.
pub const MAX_PAGE_SIZE: usize = 32768;

/// 1: the page's log sequence number, a little-endian u64.
pub const LSN: usize = 1;

/// 9: where each kind's own layout starts, after the kind byte and the LSN.
pub const BODY: usize = 9;

/// Bytes at the end of every page that hold its checksum: one u32.
pub const CHECKSUM_LEN: usize = 4;

/// Checks that `size` is a page size a file can use, and returns it as a
/// `usize`.
///
/// # Errors
///
/// [`Error::BadPageSize`] if `size` is not a power of two from
/// [`MIN_PAGE_SIZE`] to [`MAX_PAGE_SIZE`].
///
/// # Examples
///
/// ```
/// use pagefile::page::check_page_size;
///
/// assert_eq!(check_page_size(4096).unwrap(), 4096);
/// assert!(check_page_size(1000).is_err());
/// ```
pub fn check_page_size(size: u32) -> Result<usize> {
    let bytes = size as usize;
    if size.is_power_of_two() && (MIN_PAGE_SIZE..=MAX_PAGE_SIZE).contains(&bytes) {
        Ok(bytes)
    } else {
        Err(crate::Error::BadPageSize(size))
    }
}

/// The number of a page, counting from 0 at the start of the file.
///
/// Page 0 is the header, and nothing ever points to it, so a `PageNo` is
/// never 0. That leaves 0 free to mean "no page" on disk, and keeps
/// `Option<PageNo>` at 4 bytes in memory.
///
/// # Examples
///
/// ```
/// use pagefile::PageNo;
///
/// assert_eq!(PageNo::new(0), None);
/// let seven = PageNo::new(7).unwrap();
/// assert_eq!(seven.get(), 7);
/// assert_eq!(seven.to_string(), "7");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PageNo(NonZeroU32);

impl PageNo {
    /// Makes the page number `n`, or returns `None` when `n` is 0, because
    /// a `PageNo` cannot name the header.
    pub fn new(n: u32) -> Option<PageNo> {
        NonZeroU32::new(n).map(PageNo)
    }

    /// The page number as a plain integer.
    pub fn get(self) -> u32 {
        self.0.get()
    }
}

impl fmt::Display for PageNo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.get())
    }
}

/// Turns a link to another page into the u32 stored on disk: the page
/// number, or 0 for no page. [`PageNo::new`] turns it back.
///
/// # Examples
///
/// ```
/// use pagefile::PageNo;
/// use pagefile::page::link_to_u32;
///
/// assert_eq!(link_to_u32(None), 0);
/// assert_eq!(link_to_u32(PageNo::new(9)), 9);
/// ```
pub fn link_to_u32(link: Option<PageNo>) -> u32 {
    link.map_or(0, PageNo::get)
}

/// What a page holds, from its first byte. Page 0 has no kind byte; it
/// starts with the magic string instead.
///
/// Each variant's value is the byte stored on disk. A zeroed page reads as
/// [`Empty`](PageKind::Empty). That is on purpose: a page that was added to
/// the file and never written is all zeros, and it reads as a page with
/// nothing in it, not as garbage.
///
/// # Examples
///
/// ```
/// use pagefile::PageKind;
///
/// assert_eq!(PageKind::Slotted as u8, 2);
/// assert_eq!(PageKind::from_byte(0), Some(PageKind::Empty));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageKind {
    /// Allocated but not written yet: all zeros apart from the LSN and the
    /// checksum.
    Empty = 0,
    /// On the freelist, waiting to be reused.
    Free = 1,
    /// Variable-size items in the slotted format.
    Slotted = 2,
    /// One piece of a value too big for a page.
    Overflow = 3,
}

impl PageKind {
    /// The kind stored as `byte`, or `None` if no kind uses that byte. A
    /// damaged or foreign page can have any byte here.
    ///
    /// # Examples
    ///
    /// ```
    /// use pagefile::PageKind;
    ///
    /// assert_eq!(PageKind::from_byte(2), Some(PageKind::Slotted));
    /// assert_eq!(PageKind::from_byte(200), None);
    /// ```
    pub fn from_byte(byte: u8) -> Option<PageKind> {
        match byte {
            0 => Some(PageKind::Empty),
            1 => Some(PageKind::Free),
            2 => Some(PageKind::Slotted),
            3 => Some(PageKind::Overflow),
            _ => None,
        }
    }

    /// The kind's name in lowercase, for messages and reports, such as
    /// `"slotted"`.
    pub fn name(self) -> &'static str {
        match self {
            PageKind::Empty => "empty",
            PageKind::Free => "free",
            PageKind::Slotted => "slotted",
            PageKind::Overflow => "overflow",
        }
    }
}

/// The bytes of one page, in memory.
///
/// It is a fixed-size buffer with helpers to read and write little-endian
/// numbers at byte offsets, and to set and check the checksum. The modules
/// for each kind of page read and write their own layout through it.
///
/// It does not know its page number or which file it came from. So the
/// same buffer can be used for any page: read a page into it, change it,
/// and write it as any page number. No field in it has to be updated when
/// it is reused. The caller passes the page number to each call that needs
/// it.
///
/// # Examples
///
/// ```
/// use pagefile::{Page, PageKind};
///
/// let mut page = Page::new(512);
/// assert_eq!(page.kind(), Some(PageKind::Empty));
/// page.reset(PageKind::Slotted);
/// page.put_u32(20, 0xCAFE);
/// assert_eq!(page.u32_at(20), 0xCAFE);
/// assert_eq!(page.bytes()[20..24], [0xFE, 0xCA, 0, 0]);
/// ```
pub struct Page {
    bytes: Box<[u8]>,
}

impl Page {
    /// Makes a page of `size` zero bytes, so its kind is `Empty`. Any size
    /// works here, but the rest of the crate expects one that passed
    /// [`check_page_size`].
    pub fn new(size: usize) -> Page {
        Page {
            bytes: vec![0; size].into_boxed_slice(),
        }
    }

    /// The page size, in bytes, checksum included.
    pub fn size(&self) -> usize {
        self.bytes.len()
    }

    /// Every byte of the page, from the kind byte to the checksum.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Every byte of the page, for changing. Changing any byte makes the
    /// stored checksum wrong until [`seal`](Self::seal) is called.
    pub fn bytes_mut(&mut self) -> &mut [u8] {
        &mut self.bytes
    }

    /// The kind byte, decoded, or `None` if the byte matches no kind. On
    /// page 0 this reads the first letter of the magic string, so it means
    /// nothing there.
    pub fn kind(&self) -> Option<PageKind> {
        PageKind::from_byte(self.bytes[0])
    }
    /// Checks that this page has kind `kind`. `n` is the page's number,
    /// for the error.
    ///
    /// Code that follows a link to a page calls this before it trusts the
    /// page's layout. A damaged link then gives an error that names the
    /// page, instead of reading another format's bytes as its own.
    ///
    /// # Errors
    ///
    /// [`Error::WrongKind`] if the kind byte is anything else.
    ///
    /// # Examples
    ///
    /// ```
    /// use pagefile::{Error, Page, PageKind, PageNo};
    ///
    /// let page = Page::new(512);
    /// let n = PageNo::new(3).unwrap();
    /// assert!(page.expect_kind(n, PageKind::Empty).is_ok());
    /// let e = page.expect_kind(n, PageKind::Free).unwrap_err();
    /// assert!(matches!(e, Error::WrongKind { page: 3, .. }));
    /// ```
    pub fn expect_kind(&self, n: PageNo, kind: PageKind) -> Result<()> {
        if self.kind() == Some(kind) {
            return Ok(());
        }
        Err(Error::WrongKind {
            page: n.get(),
            expected: kind,
            found: self.bytes[0],
        })
    }

    /// Makes this an empty page of kind `kind`: the kind byte is set, and
    /// every byte after the LSN, checksum included, becomes zero. The LSN
    /// stays, because a page's LSN only ever moves forward.
    ///
    /// # Examples
    ///
    /// ```
    /// use pagefile::{Page, PageKind};
    ///
    /// let mut page = Page::new(512);
    /// page.set_lsn(7);
    /// page.put_u32(100, 1);
    /// page.reset(PageKind::Free);
    /// assert_eq!(page.kind(), Some(PageKind::Free));
    /// assert_eq!((page.lsn(), page.u32_at(100)), (7, 0));
    /// ```
    pub fn reset(&mut self, kind: PageKind) {
        let lsn = self.lsn();
        self.bytes.fill(0);
        self.bytes[0] = kind as u8;
        self.set_lsn(lsn);
    }
    /// The page's log sequence number: the position in the log of the last
    /// change to this page. The log belongs to the code above this crate.
    /// 0 means no logged change. This crate stores it and never reads it.
    /// On page 0 these bytes are part of the header, so it means nothing.
    pub fn lsn(&self) -> u64 {
        self.u64_at(LSN)
    }

    /// Sets the page's log sequence number. [`reset`](Self::reset) keeps it.
    pub fn set_lsn(&mut self, lsn: u64) {
        self.put_u64(LSN, lsn);
    }
    /// Reads the little-endian u16 at bytes `at` and `at + 1`.
    ///
    /// # Panics
    ///
    /// This and the other number helpers panic if the number would run past
    /// the end of the page.
    pub fn u16_at(&self, at: usize) -> u16 {
        let b = &self.bytes;
        u16::from_le_bytes([b[at], b[at + 1]])
    }

    /// Writes `value` as a little-endian u16 at byte `at`.
    pub fn put_u16(&mut self, at: usize, value: u16) {
        self.bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
    }

    /// Reads the little-endian u32 at byte `at`.
    pub fn u32_at(&self, at: usize) -> u32 {
        let b = &self.bytes;
        u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
    }

    /// Writes `value` as a little-endian u32 at byte `at`.
    pub fn put_u32(&mut self, at: usize, value: u32) {
        self.bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    /// Reads the little-endian u64 at byte `at`.
    pub fn u64_at(&self, at: usize) -> u64 {
        let mut b = [0; 8];
        b.copy_from_slice(&self.bytes[at..at + 8]);
        u64::from_le_bytes(b)
    }

    /// Writes `value` as a little-endian u64 at byte `at`.
    pub fn put_u64(&mut self, at: usize, value: u64) {
        self.bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
    }
    /// How many bytes a page's format can use: everything before the
    /// checksum. It is also the offset where the checksum starts.
    pub fn usable(&self) -> usize {
        self.size() - CHECKSUM_LEN
    }

    /// Computes the CRC-32C of the usable bytes and stores it in the last 4
    /// bytes.
    ///
    /// Code that writes through a page file does not need to call it:
    /// `PageFile::write_page` and every `PageStore::write` seal the page as
    /// they write it. It is public for code that writes page bytes some
    /// other way, and for tests.
    ///
    /// # Examples
    ///
    /// ```
    /// use pagefile::Page;
    ///
    /// let mut page = Page::new(512);
    /// page.put_u32(100, 7);
    /// assert!(page.verify(1).is_err());
    /// page.seal();
    /// assert!(page.verify(1).is_ok());
    /// ```
    pub fn seal(&mut self) {
        let end = self.usable();
        let sum = crc32c(&self.bytes[..end]);
        self.put_u32(end, sum);
    }

    /// Checks the stored checksum against the bytes. `number` is the page's
    /// number, for the error message.
    ///
    /// # Errors
    ///
    /// [`Error::Checksum`] if they do not match: the page was damaged, or
    /// changed and not sealed. An all-zero page fails too, because the
    /// CRC-32C of zeros is not zero.
    pub fn verify(&self, number: u32) -> Result<()> {
        let end = self.usable();
        let stored = self.u32_at(end);
        let computed = crc32c(&self.bytes[..end]);
        if stored == computed {
            Ok(())
        } else {
            Err(Error::Checksum {
                page: number,
                stored,
                computed,
            })
        }
    }
}

#[test]
fn page_sizes() {
    for size in [512, 1024, 4096, 32768] {
        assert_eq!(check_page_size(size).unwrap(), size as usize);
    }
    for size in [0, 256, 1000, 65536] {
        assert!(check_page_size(size).is_err(), "{size} should be rejected");
    }
}

#[test]
fn page_numbers() {
    assert_eq!(PageNo::new(0), None);
    assert_eq!(PageNo::new(7).map(PageNo::get), Some(7));
    assert_eq!(size_of::<Option<PageNo>>(), 4);
}
