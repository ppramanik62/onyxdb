//! `PageFile`: a page file on disk.
//!
//! It creates or opens the file through a `vfs::Vfs`, reads and writes whole
//! pages at `page number × page size`, checks every page's checksum, and
//! keeps a copy of the header in page 0 in memory.
//!
//! There are two ways to reach a page. `read_page` and `write_page` take
//! any page number and check only the checksum. The `PageStore` methods
//! `read` and `write` also check the number against the header's page
//! count, and most callers use those.
//!
//! The unchecked pair is for code that keeps its own copy of the header,
//! such as a cache in front of the file. That copy can count new pages that
//! the header on disk does not count yet, and the cache must be able to
//! write those pages. The cache checks page numbers against its own copy of
//! the header instead.
//!
//! Only `create` syncs. Every other function leaves that to the caller,
//! which calls `sync` when its writes must be durable. A sync waits for the
//! disk and can take milliseconds. If every write synced, every write would
//! take that long. The caller knows which writes belong together, so it can
//! sync once after all of them. `create` is different because a new file
//! cannot be used until its header is on the disk. If a crash lost the
//! header, the next `open` would find a file with no header and fail.

use std::path::Path;

use vfs::{File, Open, Vfs};

use crate::{
    Error, Header, Page, PageNo, Result,
    header::{PREFIX_LEN, USER_LEN, page_size_from_prefix},
    page::check_page_size,
    store::PageStore,
};

/// An open page file.
///
/// It owns the open file and a copy of the header in memory. Every change to
/// the copy is written to page 0 immediately, so reading the header never
/// reads the disk.
///
/// Because the header is written on every change, and not when the file is
/// closed, the copy and page 0 always match, and no change can be forgotten.
/// The cost is one page write for every change to the header.
///
/// # Examples
///
/// ```
/// use pagefile::{PageFile, PageNo, PageStore};
/// use vfs::Mem;
///
/// let mem = Mem::new();
/// let mut file = PageFile::create(&mem, "db", 512).unwrap();
///
/// // Make room for page 1, then write it.
/// let mut header = file.read_header().unwrap();
/// header.page_count = 2;
/// file.write_header(&header).unwrap();
/// let one = PageNo::new(1).unwrap();
/// let mut page = file.new_page();
/// page.set_lsn(5);
/// file.write(one, &mut page).unwrap();
/// file.sync().unwrap();
///
/// let file = PageFile::open(&mem, "db").unwrap();
/// let mut page = file.new_page();
/// file.read(one, &mut page).unwrap();
/// assert_eq!(page.lsn(), 5);
/// ```
pub struct PageFile {
    file: Box<dyn File>,
    header: Header,
}

impl PageFile {
    /// The byte offset where page `n` starts: `n` times the page size, as
    /// a u64, because a large file passes 4 GiB.
    fn offset(&self, n: u32) -> u64 {
        n as u64 * self.header.page_size as u64
    }

    /// Reads page `n` into `page` and checks its checksum. It does not check
    /// `n` against the page count: `read` does, and a cache in front of the
    /// file checks it against its own header.
    ///
    /// It takes the page number as a plain `u32`, not a `PageNo`, so it can
    /// also read page 0.
    ///
    /// # Errors
    ///
    /// - [`Error::Vfs`] if the read fails. A page past the end of the file
    ///   is a `vfs::Error::ShortRead`.
    /// - [`Error::Checksum`] if the page's bytes do not match its checksum.
    ///   `page` holds the bad bytes then.
    ///
    /// # Panics
    ///
    /// Panics if `page` is not the file's page size.
    pub fn read_page(&self, n: u32, page: &mut Page) -> Result<()> {
        assert_eq!(
            page.size(),
            self.header.page_size as usize,
            "the page buffer has the wrong size"
        );
        self.file.read_at(page.bytes_mut(), self.offset(n))?;
        page.verify(n)
    }

    /// Sets the checksum of `page` and writes it as page `n`. Like
    /// `read_page`, it does not check `n` against the page count, so it can
    /// write past the end and make the file longer. It does not sync.
    ///
    /// # Errors
    ///
    /// [`Error::Vfs`] if the write fails. Part of the page may have been
    /// written then.
    ///
    /// # Panics
    ///
    /// Panics if `page` is not the file's page size.
    ///
    /// # Examples
    ///
    /// ```
    /// use pagefile::PageFile;
    /// use vfs::Mem;
    ///
    /// let mem = Mem::new();
    /// let file = PageFile::create(&mem, "db", 512).unwrap();
    /// // Page 5 is past the page count, and write_page writes it anyway.
    /// let mut page = pagefile::Page::new(512);
    /// page.put_u32(100, 42);
    /// file.write_page(5, &mut page).unwrap();
    /// assert_eq!(file.file_len().unwrap(), 6 * 512);
    ///
    /// let mut back = pagefile::Page::new(512);
    /// file.read_page(5, &mut back).unwrap();
    /// assert_eq!(back.u32_at(100), 42);
    /// ```
    pub fn write_page(&self, n: u32, page: &mut Page) -> Result<()> {
        assert_eq!(
            page.size(),
            self.header.page_size as usize,
            "the page buffer has the wrong size"
        );
        page.seal();
        self.file.write_at(page.bytes(), self.offset(n))?;
        Ok(())
    }

    /// Encodes the header in memory and writes it as page 0, checksum
    /// included.
    fn save_header(&self) -> Result<()> {
        let mut page = Page::new(self.header.page_size as usize);
        self.header.encode(&mut page);
        self.write_page(0, &mut page)
    }
    /// Checks that page `n` is below the header's page count. `read` and
    /// `write` call it, so they return [`Error::OutOfRange`] for a page the
    /// file does not have.
    fn check(&self, n: PageNo) -> Result<()> {
        if n.get() >= self.header.page_count {
            return Err(Error::OutOfRange {
                page: n.get(),
                count: self.header.page_count,
            });
        }
        Ok(())
    }

    /// The copy of the header in memory, as of the last change. It matches
    /// page 0 unless the last write of the header failed.
    pub fn header(&self) -> &Header {
        &self.header
    }

    /// The number of pages in the file, page 0 included. Pages 1 to
    /// `page_count() - 1` can be read and written.
    pub fn page_count(&self) -> u32 {
        self.header.page_count
    }

    /// The file's length on disk, in bytes. It can be more than the page
    /// count times the page size, for example after a crash in the middle of
    /// adding pages.
    ///
    /// # Errors
    ///
    /// [`Error::Vfs`] if the file system cannot say.
    pub fn file_len(&self) -> Result<u64> {
        Ok(self.file.len()?)
    }

    /// Replaces the header's user bytes and writes the header to page 0. It
    /// does not sync.
    ///
    /// # Examples
    ///
    /// ```
    /// use pagefile::PageFile;
    /// use vfs::Mem;
    ///
    /// let mem = Mem::new();
    /// let mut file = PageFile::create(&mem, "db", 512).unwrap();
    /// let mut user = [0; 32];
    /// user[0] = 7;
    /// file.set_user(user).unwrap();
    /// file.sync().unwrap();
    /// assert_eq!(PageFile::open(&mem, "db").unwrap().header().user[0], 7);
    /// ```
    ///
    /// # Errors
    ///
    /// [`Error::Vfs`] if the write fails. The copy in memory has the new
    /// bytes even then.
    pub fn set_user(&mut self, user: [u8; USER_LEN]) -> Result<()> {
        self.header.user = user;
        self.save_header()
    }
    /// Creates a page file at `path` in `vfs` that holds only the header
    /// page, and makes it durable: the file and its name are both synced.
    ///
    /// # Errors
    ///
    /// - [`Error::BadPageSize`] if `page_size` is not a power of two from
    ///   512 to 32768. Nothing is created then.
    /// - [`Error::Vfs`] if a file is already at `path`
    ///   (`vfs::Error::AlreadyExists`), or if creating, writing or syncing
    ///   fails.
    ///
    /// `path` takes `impl AsRef<Path>`, so a `&str`, a `&Path` or a
    /// `PathBuf` all work.
    ///
    /// # Examples
    ///
    /// ```
    /// use pagefile::{Error, PageFile};
    /// use vfs::Mem;
    ///
    /// let mem = Mem::new();
    /// let file = PageFile::create(&mem, "db", 4096).unwrap();
    /// assert_eq!((file.page_count(), file.file_len().unwrap()), (1, 4096));
    ///
    /// assert!(matches!(PageFile::create(&mem, "other", 1000), Err(Error::BadPageSize(1000))));
    /// assert!(matches!(PageFile::create(&mem, "db", 4096), Err(Error::Vfs(_))));
    /// ```
    pub fn create(vfs: &dyn Vfs, path: impl AsRef<Path>, page_size: u32) -> Result<PageFile> {
        let size = check_page_size(page_size)?;
        let path = path.as_ref();
        let file = vfs.open(path, Open::CreateNew)?;
        let file = PageFile {
            file,
            header: Header::new(size),
        };
        file.save_header()?;
        file.sync()?;
        vfs.sync_dir(path.parent().unwrap_or(Path::new("")))?;
        Ok(file)
    }

    /// Opens an existing page file at `path` in `vfs` and checks its header.
    /// The other pages are not read until they are asked for.
    ///
    /// # Errors
    ///
    /// - [`Error::Vfs`] if nothing is at `path`, or a read fails.
    /// - [`Error::NotAPageFile`] if the file is shorter than the header's
    ///   first [`PREFIX_LEN`] bytes or does not start with the magic string.
    /// - [`Error::UnsupportedVersion`] or [`Error::BadPageSize`] if the
    ///   header holds a version or page size this code cannot use.
    /// - [`Error::Checksum`] if page 0 is damaged.
    /// - [`Error::Truncated`] if the file is shorter than one page, or than
    ///   the page count times the page size.
    ///
    /// `open` checks the length once, because a crash while pages were
    /// being added can leave the file shorter than its header says. Without
    /// this check, the problem would appear later, as a failed read of some
    /// page in the middle of other work. At open, it is one clear error.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::path::Path;
    /// use pagefile::{Error, PageFile};
    /// use vfs::{Mem, Open, Vfs};
    ///
    /// let mem = Mem::new();
    /// PageFile::create(&mem, "db", 512).unwrap();
    /// assert_eq!(PageFile::open(&mem, "db").unwrap().page_count(), 1);
    ///
    /// // Cut the file short, as a crash might.
    /// mem.open(Path::new("db"), Open::Existing).unwrap().set_len(300).unwrap();
    /// assert!(matches!(PageFile::open(&mem, "db"), Err(Error::Truncated { len: 300, .. })));
    /// ```
    pub fn open(vfs: &dyn Vfs, path: impl AsRef<Path>) -> Result<PageFile> {
        let file = vfs.open(path.as_ref(), Open::Existing)?;
        let len = file.len()?;
        if len < PREFIX_LEN as u64 {
            return Err(Error::NotAPageFile);
        }
        // The page size is stored inside page 0, so read just enough of page 0
        // to learn it before reading the whole page.
        let mut prefix = [0; PREFIX_LEN];
        file.read_at(&mut prefix, 0)?;
        let size = page_size_from_prefix(&prefix)?;
        if len < size as u64 {
            return Err(Error::Truncated {
                len,
                expected: size as u64,
            });
        }
        let mut page = Page::new(size);
        file.read_at(page.bytes_mut(), 0)?;
        page.verify(0)?;
        let header = Header::decode(&page)?;
        let expected = header.page_count as u64 * size as u64;
        if len < expected {
            return Err(Error::Truncated { len, expected });
        }
        Ok(PageFile { file, header })
    }

    /// Waits until every write to this file so far is on the disk. Until
    /// then, a crash can lose any of them, or tear a page in half.
    ///
    /// # Errors
    ///
    /// [`Error::Vfs`] if the sync fails. Then nothing is known about which
    /// writes reached the disk.
    pub fn sync(&self) -> Result<()> {
        self.file.sync()?;
        Ok(())
    }
}

/// The file itself as a [`PageStore`]: pages are read from and written to
/// the file, and the header is the copy in memory, written to page 0 on
/// every change. `read_header` never fails, because it only clones that
/// copy.
impl PageStore for PageFile {
    type Error = Error;

    fn page_size(&self) -> usize {
        self.header.page_size as usize
    }

    fn read_header(&self) -> Result<Header> {
        Ok(self.header.clone())
    }

    fn write_header(&mut self, header: &Header) -> Result<()> {
        self.header = header.clone();
        self.save_header()
    }

    fn read(&self, n: PageNo, page: &mut Page) -> Result<()> {
        self.check(n)?;
        self.read_page(n.get(), page)
    }

    fn write(&mut self, n: PageNo, page: &mut Page) -> Result<()> {
        self.check(n)?;
        self.write_page(n.get(), page)
    }
}
