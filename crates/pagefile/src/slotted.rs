//! Slotted pages: many variable-size items in one page.
//!
//! ```text
//! 0      1     9     10      12              14        16
//! +------+-----+-----+-------+---------------+---------+--------+--
//! | kind | LSN | tag | count | content start | garbage | slot 0 | ...
//! +------+-----+-----+-------+---------------+---------+--------+--
//!
//!   --+--------+------------+--------+---------+--------+----------+
//!  ...| slot n |  free gap  | item 1 | garbage | item 0 | checksum |
//!   --+--------+------------+--------+---------+--------+----------+
//!              ^            ^                           ^
//!       16 + 4 x count  content start               size - 4
//! ```
//!
//! | Term | Meaning |
//! |---|---|
//! | item | A byte string stored in the page. Items are numbered from 0 in slot order. |
//! | slot | An item's offset and length, two little-endian u16s. |
//! | item area | From content start to the checksum: the items and the garbage. |
//! | free gap | The free bytes between the last slot and the item area. |
//! | garbage | Bytes in the item area that belong to no item, left by removing or shrinking one. |
//! | room | The free gap plus the garbage: what is free once the page is compacted. |
//! | tag | One byte in the header that the page's owner uses as it likes. |
//!
//! The slots are in the order the owner of the page chooses, and inserting
//! at position `i` shifts the slots after it. Items are packed from the end
//! of the page toward the slots, so the free gap is always in one piece in
//! the middle. A removed item's bytes stay where they were, counted as
//! garbage, until `compact` packs the items again.
//!
//! Item numbers change when items are inserted or removed, like indexes into
//! a `Vec`. An owner that needs a name for an item that never changes stores
//! that name inside the item.
//!
//! Why this layout:
//!
//! - The owner picks the slot order so it can keep items sorted, for example
//!   by key, and find one with a binary search over the slots. Inserting in
//!   the middle shifts the slots after it, but a slot is only 4 bytes, so
//!   that is a small copy. The items themselves never move on insert.
//! - Slots grow from the front and items from the back, so neither needs a
//!   fixed share of the page. A page of many small items uses more of it for
//!   slots, and a page of a few big items uses more for items.
//! - Removing an item leaves its bytes as garbage. The page is not packed
//!   immediately, because packing copies every item, and packing on every
//!   remove would copy the page every time. The garbage is reused later, in
//!   one copy, and only when an insert needs the space.
//! - Offsets and lengths are u16s, 2 bytes each. That is enough because the
//!   largest page is 32768 bytes, and it keeps a slot at 4 bytes.

use std::ops::{Deref, DerefMut};

use crate::{Error, Page, PageKind, Result, page::CHECKSUM_LEN};

/// 9: a byte the page's owner can use, for example to say what the page holds.
const TAG: usize = 9;
/// 10: the number of slots.
const COUNT: usize = 10;
/// 12: where the item area starts. From here to the checksum is items and
/// garbage.
const CONTENT: usize = 12;
/// 14: how many bytes in the item area belong to no item.
const GARBAGE: usize = 14;

/// The size of the slotted page header, the kind byte and the LSN included.
/// The first slot starts here.
pub const HEADER_LEN: usize = 16;

/// The size of one slot: an offset and a length.
pub const SLOT_LEN: usize = 4;

/// The largest item a slotted page of `page_size` bytes can hold: the whole
/// page except the header, one slot and the checksum.
///
/// # Examples
///
/// ```
/// use pagefile::slotted::max_item;
///
/// // 512 - 4 for the checksum - 16 for the header - 4 for one slot.
/// assert_eq!(max_item(512), 488);
/// ```
pub fn max_item(page_size: usize) -> usize {
    page_size - CHECKSUM_LEN - HEADER_LEN - SLOT_LEN
}

/// A page seen through the slotted format. `B` is `&Page` for reading or
/// `&mut Page` for changing it.
///
/// The read methods need `B: Deref<Target = Page>`, which both `&Page` and
/// `&mut Page` meet: anything that can be read as a `Page`. The methods
/// that change the page need `B: DerefMut<Target = Page>`, which only
/// `&mut Page` meets. So one type covers reading and changing, and the
/// compiler stops a change through a shared borrow.
///
/// It is a view: it holds the page, and it reads and writes the slotted
/// layout in the page's own bytes, with no copy. It is not a decoded struct,
/// so there is nothing to encode afterward. Every change is already in the
/// page's bytes, so writing the page writes the change.
/// [`init`](Self::init) formats a page, and [`open`](Self::open) checks a
/// page that is already formatted. Dropping the view leaves the changes in
/// the page. The checksum is set when the page is written.
///
/// # Examples
///
/// ```
/// use pagefile::{Page, Slotted};
///
/// let mut page = Page::new(512);
/// let mut items = Slotted::init(&mut page, 0);
/// items.insert(0, b"cherry").unwrap();
/// items.insert(0, b"apple").unwrap();
/// items.insert(1, b"banana").unwrap();
/// assert_eq!(items.get(1), Some(&b"banana"[..]));
///
/// items.remove(0);
/// assert_eq!(items.get(0), Some(&b"banana"[..]));
/// assert_eq!(items.garbage(), 5);
///
/// // Reading needs only a shared borrow.
/// let items = Slotted::open(&page).unwrap();
/// assert_eq!(items.len(), 2);
/// ```
pub struct Slotted<B> {
    page: B,
}

impl<B: Deref<Target = Page>> Slotted<B> {
    /// Opens a page that is already in the slotted format, after checking that
    /// its header and every slot stay inside the page. After that, no read
    /// through the view can go out of bounds, even if the page came from a
    /// damaged file.
    ///
    /// It reads every slot, so its time grows with the number of items. In
    /// return, no later read needs a bounds check of its own. A damaged page
    /// fails here, with an error that says which numbers disagree, and does
    /// not cause a panic later.
    ///
    /// # Errors
    ///
    /// [`Error::BadSlottedPage`] if the kind byte is not `Slotted`, if the
    /// slots and the item area overlap, if any slot points outside the item
    /// area, or if the items and the garbage do not add up to the item
    /// area.
    ///
    /// # Examples
    ///
    /// ```
    /// use pagefile::{Error, Page, Slotted};
    ///
    /// let mut page = Page::new(512);
    /// Slotted::init(&mut page, 0).insert(0, b"apple").unwrap();
    /// assert_eq!(Slotted::open(&page).unwrap().len(), 1);
    ///
    /// // Damage slot 0's length (bytes 18 and 19) so it runs past the page.
    /// page.put_u16(18, 600);
    /// assert!(matches!(Slotted::open(&page), Err(Error::BadSlottedPage(_))));
    /// ```
    pub fn open(page: B) -> Result<Slotted<B>> {
        if page.kind() != Some(PageKind::Slotted) {
            let found = page.bytes()[0];
            return Err(Error::BadSlottedPage(format!(
                "the kind byte is {found}, not 2"
            )));
        }
        let s = Slotted { page };
        let (slots_end, content, end) = (s.slots_end(), s.content_start(), s.page.usable());
        if slots_end > content || content > end {
            let why = format!(
                "slots end at {slots_end} and items start at {content}, in a page whose items must end by {end}"
            );
            return Err(Error::BadSlottedPage(why));
        }
        let mut used = 0;
        for i in 0..s.len() {
            let (offset, len) = s.slot(i);
            if offset < content || offset + len > end {
                let why = format!(
                    "slot {i} covers bytes {offset}..{}, outside the item area {content}..{end}",
                    offset + len
                );
                return Err(Error::BadSlottedPage(why));
            }
            used += len;
        }
        if used + s.garbage() != end - content {
            let why = format!(
                "items use {used} bytes and garbage {}, but the item area is {} bytes",
                s.garbage(),
                end - content
            );
            return Err(Error::BadSlottedPage(why));
        }
        Ok(s)
    }

    /// The number of items, which is also the number of slots.
    pub fn len(&self) -> usize {
        self.page.u16_at(COUNT) as usize
    }

    /// Whether the page holds no items.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The owner's tag byte, as set by [`init`](Self::init) or
    /// [`set_tag`](Self::set_tag).
    pub fn tag(&self) -> u8 {
        self.page.bytes()[TAG]
    }

    /// The bytes of item `i`, borrowed from the page, or `None` if `i` is
    /// past the last item.
    pub fn get(&self, i: usize) -> Option<&[u8]> {
        if i >= self.len() {
            return None;
        }
        let (offset, len) = self.slot(i);
        Some(&self.page.bytes()[offset..offset + len])
    }

    /// Bytes free for new items and their slots. This includes the garbage,
    /// which compaction turns back into free space. An item of `n` bytes
    /// fits if `n + SLOT_LEN <= room()`.
    pub fn room(&self) -> usize {
        self.gap() + self.garbage()
    }

    /// The offset and length stored in slot `i`, in bytes.
    ///
    /// # Panics
    ///
    /// Panics if `i` is far enough past the last slot to run off the page.
    /// A slot past the last item, but still inside the page, returns
    /// whatever bytes are there.
    pub fn slot(&self, i: usize) -> (usize, usize) {
        let at = HEADER_LEN + i * SLOT_LEN;
        (
            self.page.u16_at(at) as usize,
            self.page.u16_at(at + 2) as usize,
        )
    }

    /// The offset where the item area starts. Every item is at or after it.
    pub fn content_start(&self) -> usize {
        self.page.u16_at(CONTENT) as usize
    }

    /// How many bytes of the item area belong to no item. They become free
    /// again when the page is compacted.
    pub fn garbage(&self) -> usize {
        self.page.u16_at(GARBAGE) as usize
    }

    /// The first byte after the last slot.
    fn slots_end(&self) -> usize {
        HEADER_LEN + self.len() * SLOT_LEN
    }

    /// The free bytes between the last slot and the start of the item area.
    /// A new item and its slot can go here without compacting.
    fn gap(&self) -> usize {
        self.content_start() - self.slots_end()
    }
}

impl<B: DerefMut<Target = Page>> Slotted<B> {
    /// Formats `page` as an empty slotted page with the given tag, erasing
    /// what was there except the LSN. The whole page after the header is
    /// the free gap.
    ///
    /// # Examples
    ///
    /// ```
    /// use pagefile::{Page, Slotted};
    /// use pagefile::slotted::max_item;
    ///
    /// let mut page = Page::new(512);
    /// let items = Slotted::init(&mut page, 3);
    /// assert_eq!((items.len(), items.tag()), (0, 3));
    /// // Room for one item of the largest size, and its slot.
    /// assert_eq!(items.room(), max_item(512) + 4);
    /// ```
    pub fn init(mut page: B, tag: u8) -> Slotted<B> {
        page.reset(PageKind::Slotted);
        page.bytes_mut()[TAG] = tag;
        let end = page.usable() as u16;
        page.put_u16(CONTENT, end);
        Slotted { page }
    }

    /// Changes the owner's tag byte. Nothing else in the page changes.
    pub fn set_tag(&mut self, tag: u8) {
        self.page.bytes_mut()[TAG] = tag;
    }

    /// Writes `offset` and `len` into slot `i`. It does not change the
    /// count, so the caller makes room for the slot first.
    fn set_slot(&mut self, i: usize, offset: usize, len: usize) {
        let at = HEADER_LEN + i * SLOT_LEN;
        self.page.put_u16(at, offset as u16);
        self.page.put_u16(at + 2, len as u16);
    }

    /// Inserts `item` as item `i`, so items `i` and later become `i + 1` and
    /// later. Compacts the page first if that is the only way the item fits.
    ///
    /// Only the slots move. The item's bytes go at the front of the item
    /// area, wherever `i` is. A compaction copies the whole page.
    ///
    /// # Errors
    ///
    /// [`Error::PageFull`] if the item and its slot need more than
    /// [`room`](Self::room). The page is unchanged then.
    ///
    /// # Panics
    ///
    /// Panics if `i > len()`, like `Vec::insert`.
    ///
    /// # Examples
    ///
    /// ```
    /// use pagefile::{Page, Slotted};
    ///
    /// let mut page = Page::new(512);
    /// let mut items = Slotted::init(&mut page, 0);
    /// let big = [7; 300];
    /// items.insert(0, &big).unwrap();
    /// // 300 + 4 bytes are used, so another 300 don't fit.
    /// assert!(items.insert(1, &big).is_err());
    /// assert_eq!(items.len(), 1);
    /// ```
    pub fn insert(&mut self, i: usize, item: &[u8]) -> Result<()> {
        assert!(
            i <= self.len(),
            "insert at {i}, but the page has {} items",
            self.len()
        );
        let need = item.len() + SLOT_LEN;
        if need > self.room() {
            return Err(Error::PageFull {
                need,
                room: self.room(),
            });
        }
        if need > self.gap() {
            self.compact();
        }
        let offset = self.content_start() - item.len();
        self.page.bytes_mut()[offset..offset + item.len()].copy_from_slice(item);
        self.page.put_u16(CONTENT, offset as u16);
        // Move slots i.. one place to the right, which frees slot i.
        let at = HEADER_LEN + i * SLOT_LEN;
        let end = self.slots_end();
        self.page.bytes_mut().copy_within(at..end, at + SLOT_LEN);
        self.set_slot(i, offset, item.len());
        let count = self.len() + 1;
        self.page.put_u16(COUNT, count as u16);
        Ok(())
    }

    /// Removes item `i`, so later items move down by one. Its bytes stay
    /// where they are and count as garbage until the next compaction. Its
    /// slot is freed immediately.
    ///
    /// # Panics
    ///
    /// Panics if `i >= len()`, like `Vec::remove`.
    ///
    /// # Examples
    ///
    /// ```
    /// use pagefile::{Page, Slotted};
    ///
    /// let mut page = Page::new(512);
    /// let mut items = Slotted::init(&mut page, 0);
    /// items.insert(0, b"apple").unwrap();
    /// items.insert(1, b"fig").unwrap();
    /// let room = items.room();
    ///
    /// items.remove(0);
    /// assert_eq!(items.get(0), Some(&b"fig"[..]));
    /// // The 5 bytes of "apple" are garbage now, and its 4-byte slot is free.
    /// assert_eq!(items.garbage(), 5);
    /// assert_eq!(items.room(), room + 5 + 4);
    /// ```
    pub fn remove(&mut self, i: usize) {
        assert!(
            i < self.len(),
            "remove {i}, but the page has {} items",
            self.len()
        );
        let (_, len) = self.slot(i);
        let at = HEADER_LEN + i * SLOT_LEN;
        let end = self.slots_end();
        self.page.bytes_mut().copy_within(at + SLOT_LEN..end, at);
        let count = self.len() - 1;
        self.page.put_u16(COUNT, count as u16);
        let garbage = self.garbage() + len;
        self.page.put_u16(GARBAGE, garbage as u16);
    }

    /// Replaces item `i` with `item`, keeping its position.
    ///
    /// A new item no longer than the old one is written in place, and any
    /// bytes left over count as garbage. A longer one is removed and
    /// inserted again, which may compact the page.
    ///
    /// # Errors
    ///
    /// [`Error::PageFull`] if the new item does not fit, even counting the
    /// old item's bytes as free. The page is unchanged then.
    ///
    /// # Panics
    ///
    /// Panics if `i >= len()`.
    ///
    /// # Examples
    ///
    /// ```
    /// use pagefile::{Page, Slotted};
    ///
    /// let mut page = Page::new(512);
    /// let mut items = Slotted::init(&mut page, 0);
    /// items.insert(0, b"banana").unwrap();
    ///
    /// // Shorter: written in place, and the 3 bytes left over are garbage.
    /// items.replace(0, b"fig").unwrap();
    /// assert_eq!((items.get(0), items.garbage()), (Some(&b"fig"[..]), 3));
    ///
    /// // Longer: removed and inserted again.
    /// items.replace(0, b"cherries").unwrap();
    /// assert_eq!(items.get(0), Some(&b"cherries"[..]));
    /// ```
    pub fn replace(&mut self, i: usize, item: &[u8]) -> Result<()> {
        assert!(
            i < self.len(),
            "replace {i}, but the page has {} items",
            self.len()
        );
        let (offset, len) = self.slot(i);
        if item.len() <= len {
            self.page.bytes_mut()[offset..offset + item.len()].copy_from_slice(item);
            self.set_slot(i, offset, item.len());
            let garbage = self.garbage() + len - item.len();
            self.page.put_u16(GARBAGE, garbage as u16);
            return Ok(());
        }
        // Check before changing anything. The old item's bytes and slot will
        // be reused, so they count as room.
        if item.len() > self.room() + len {
            return Err(Error::PageFull {
                need: item.len(),
                room: self.room() + len,
            });
        }
        self.remove(i);
        self.insert(i, item)
    }

    /// Packs every item against the end of the page, in slot order, so the
    /// garbage becomes part of the free gap. Item numbers and their bytes
    /// do not change, only where the bytes sit.
    ///
    /// It copies the whole page to a temporary buffer first, so it costs
    /// one page-sized allocation and copy. `insert` calls it when it has
    /// to, so callers rarely need it.
    ///
    /// The copy is needed because packing moves items toward the end of the
    /// page, and an item can be moved onto bytes that another item still
    /// uses. Each item is read from the copy, so no item is overwritten
    /// before it is moved.
    ///
    /// # Examples
    ///
    /// ```
    /// use pagefile::{Page, Slotted};
    ///
    /// let mut page = Page::new(512);
    /// let mut items = Slotted::init(&mut page, 0);
    /// items.insert(0, b"apple").unwrap();
    /// items.insert(1, b"fig").unwrap();
    /// items.remove(0);
    /// let room = items.room();
    ///
    /// items.compact();
    /// assert_eq!(items.garbage(), 0);
    /// // The garbage moved into the free gap, so the room is the same.
    /// assert_eq!(items.room(), room);
    /// assert_eq!(items.get(0), Some(&b"fig"[..]));
    /// ```
    pub fn compact(&mut self) {
        let old = self.page.bytes().to_vec();
        let mut end = self.page.usable();
        for i in 0..self.len() {
            let (offset, len) = self.slot(i);
            end -= len;
            self.page.bytes_mut()[end..end + len].copy_from_slice(&old[offset..offset + len]);
            self.set_slot(i, end, len);
        }
        self.page.put_u16(CONTENT, end as u16);
        self.page.put_u16(GARBAGE, 0);
    }
}
