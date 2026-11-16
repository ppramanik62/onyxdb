//! `PageTable`: a hash table from page number to a small value, such as the
//! frame that holds the page.
//!
//! | Term | Meaning |
//! |---|---|
//! | slot | One place in the table's array. It is empty or holds one entry. Not the same as a slot in a slotted page. |
//! | home slot | The slot a key's hash points to. A search for the key starts there. |
//! | run | A sequence of taken slots with no empty slot between them. |
//! | capacity | The most entries the table may hold, fixed when it is made. |
//!
//! Every entry lives in one array of slots. If a key's home slot is taken,
//! the key goes in the next free slot after it. So a search starts at the
//! home slot and walks forward until it finds the key or an empty slot. This
//! is open addressing with linear probing.
//!
//! ```text
//! slot   0      1      2      3      4      5      6      7
//!        empty  a      b      c      empty  empty  d      empty
//!               home 1 home 1 home 2               home 6
//!               \_______ one run ______/
//! ```
//!
//! Here keys `a` and `b` both have home slot 1. `a` got there first, so `b`
//! sits in slot 2, and `c`, whose home is slot 2, sits in slot 3. A search
//! for `c` looks at slots 2 and 3, and a search for a missing key with home
//! slot 1 looks at slots 1 to 4 and stops at the empty slot 4.
//!
//! The table never grows. It has at least twice as many slots as its
//! capacity, so it is never more than half full. That keeps runs short, and it
//! means every search ends at an empty slot.
//!
//! The pool knows the largest size of its map when it starts, because the
//! map holds at most one entry per frame. A table made for that size never
//! allocates after it is made, and its memory is fixed. Linear probing keeps
//! every entry in one array, so a search reads memory that is close
//! together, and does not follow pointers. The known problem of linear
//! probing is long runs when the table is almost full. That cannot happen
//! here, because the table is never more than half full.

use pagefile::PageNo;

/// A hash table from page number to a value of type `V`, with a fixed
/// number of slots.
///
/// The value is `Copy` and is returned by value, because the table is meant
/// for small values like a frame index. Every operation takes a few steps on
/// average, and the table never allocates after [`new`](Self::new).
///
/// # Examples
///
/// ```
/// use bufpool::PageTable;
/// use pagefile::PageNo;
///
/// let page = |n| PageNo::new(n).unwrap();
/// let mut table = PageTable::new(4);
/// assert_eq!(table.insert(page(7), 2), None);
/// assert_eq!(table.insert(page(7), 3), Some(2));
/// assert_eq!(table.get(page(7)), Some(3));
/// assert_eq!(table.remove(page(7)), Some(3));
/// assert!(table.is_empty());
/// ```
pub struct PageTable<V> {
    /// Each slot is empty or holds one entry.
    slots: Vec<Option<(PageNo, V)>>,
    /// The number of slots is 2 to the power `bits`.
    bits: u32,
    /// The most entries the table may hold: half the slots or fewer.
    capacity: usize,
    /// The number of entries.
    len: usize,
}

/// 2^64 divided by the golden ratio, rounded to an odd number. Multiplying a
/// page number by it spreads neighboring numbers far apart in the top bits.
/// Knuth calls this Fibonacci hashing.
const GOLDEN: u64 = 0x9E37_79B9_7F4A_7C15;

impl<V: Copy> PageTable<V> {
    /// Makes an empty table for up to `capacity` entries.
    ///
    /// It gets twice `capacity` slots, rounded up to a power of two, and at
    /// least 2. This is the only allocation the table ever makes.
    pub fn new(capacity: usize) -> PageTable<V> {
        let slots = (2 * capacity).max(2).next_power_of_two();
        PageTable {
            slots: vec![None; slots],
            bits: slots.trailing_zeros(),
            capacity,
            len: 0,
        }
    }

    /// The most entries the table may hold, as given to [`new`](Self::new).
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// The number of entries in the table now.
    pub fn len(&self) -> usize {
        self.len
    }

    /// True if the table has no entries.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The number of slots: twice the capacity or more, and a power of two.
    pub fn slots(&self) -> usize {
        self.slots.len()
    }

    /// The entry in slot `i`, or `None` if the slot is empty. This is for
    /// tests and tools that show how the table is laid out.
    ///
    /// # Panics
    ///
    /// Panics if `i` is not below [`slots`](Self::slots).
    pub fn slot(&self, i: usize) -> Option<(PageNo, V)> {
        self.slots[i]
    }

    /// The home slot of `key`, where a search for it starts. It is the top
    /// `bits` bits of the key times `GOLDEN`, so it is always below
    /// [`slots`](Self::slots).
    ///
    /// # Examples
    ///
    /// ```
    /// use bufpool::PageTable;
    /// use pagefile::PageNo;
    ///
    /// let table: PageTable<usize> = PageTable::new(4);
    /// let home = table.home(PageNo::new(7).unwrap());
    /// assert!(home < table.slots());
    /// ```
    pub fn home(&self, key: PageNo) -> usize {
        ((key.get() as u64).wrapping_mul(GOLDEN) >> (64 - self.bits)) as usize
    }

    /// The slot after slot `i`. After the last slot comes slot 0.
    fn next(&self, i: usize) -> usize {
        (i + 1) & (self.slots.len() - 1)
    }

    /// How many steps forward it is from slot `from` to slot `to`, wrapping
    /// around after the last slot. The distance from a slot to itself is 0.
    fn distance(&self, from: usize, to: usize) -> usize {
        to.wrapping_sub(from) & (self.slots.len() - 1)
    }
    /// The slot that holds `key`, or the empty slot where its search ends.
    /// The search always ends, because the table is never more than half
    /// full, so there is always an empty slot.
    fn find(&self, key: PageNo) -> usize {
        let mut i = self.home(key);
        while let Some((k, _)) = self.slots[i] {
            if k == key {
                break;
            }
            i = self.next(i);
        }
        i
    }

    /// The value for `key`, or `None` if the table does not have the key.
    pub fn get(&self, key: PageNo) -> Option<V> {
        self.slots[self.find(key)].map(|(_, v)| v)
    }

    /// Sets the value for `key`. Returns the old value if the key was
    /// already there, or `None` if it is new.
    ///
    /// # Panics
    ///
    /// Panics if the key is new and the table already holds
    /// [`capacity`](Self::capacity) entries. Replacing the value of a key
    /// that is already there never panics.
    ///
    /// # Examples
    ///
    /// ```
    /// use bufpool::PageTable;
    /// use pagefile::PageNo;
    ///
    /// let mut table = PageTable::new(1);
    /// table.insert(PageNo::new(3).unwrap(), 'a');
    /// // Full, but replacing a value still works.
    /// assert_eq!(table.insert(PageNo::new(3).unwrap(), 'b'), Some('a'));
    /// ```
    pub fn insert(&mut self, key: PageNo, value: V) -> Option<V> {
        let i = self.find(key);
        let old = self.slots[i].map(|(_, v)| v);
        if old.is_none() {
            assert!(
                self.len < self.capacity,
                "the table is full: it holds {} entries",
                self.capacity
            );
            self.len += 1;
        }
        self.slots[i] = Some((key, value));
        old
    }

    /// Every entry, in slot order. The order has nothing to do with the
    /// page numbers, and it changes when entries are inserted or removed.
    ///
    /// The return type `impl Iterator<Item = (PageNo, V)> + '_` means "some
    /// iterator over `(PageNo, V)` pairs" without naming its type. The `'_`
    /// says the iterator borrows the table, so the table cannot change while
    /// the iterator is in use.
    ///
    /// # Examples
    ///
    /// ```
    /// use bufpool::PageTable;
    /// use pagefile::PageNo;
    ///
    /// let mut table = PageTable::new(4);
    /// table.insert(PageNo::new(3).unwrap(), 'a');
    /// table.insert(PageNo::new(8).unwrap(), 'b');
    /// let mut entries: Vec<_> = table.iter().map(|(p, v)| (p.get(), v)).collect();
    /// entries.sort();
    /// assert_eq!(entries, [(3, 'a'), (8, 'b')]);
    /// ```
    pub fn iter(&self) -> impl Iterator<Item = (PageNo, V)> + '_ {
        self.slots.iter().flatten().copied()
    }
    /// Removes `key` and returns its value, or returns `None` if the table
    /// does not have it.
    ///
    /// Removing never leaves a gap in a run. Entries after the removed one
    /// move back, so every remaining key is still found by a search that
    /// starts at its home slot.
    ///
    /// Moving entries back costs a walk to the end of the run. A simpler way
    /// is to leave a marker in the slot that says "something was here". But
    /// the markers add up over time, and searches get longer until the table
    /// is rebuilt. With no markers, the table is always the same as if the
    /// removed key had never been inserted.
    ///
    /// # Examples
    ///
    /// ```
    /// use bufpool::PageTable;
    /// use pagefile::PageNo;
    ///
    /// let mut table = PageTable::new(4);
    /// // Find two pages with the same home slot, so the second sits after the first.
    /// let a = PageNo::new(1).unwrap();
    /// let b = (2..).map(|n| PageNo::new(n).unwrap()).find(|&p| table.home(p) == table.home(a)).unwrap();
    /// table.insert(a, 'a');
    /// table.insert(b, 'b');
    ///
    /// // Removing `a` empties the slot `b`'s search starts at, and `b` is still found.
    /// assert_eq!(table.remove(a), Some('a'));
    /// assert_eq!(table.get(b), Some('b'));
    /// assert_eq!(table.slot(table.home(b)), Some((b, 'b')));
    /// ```
    pub fn remove(&mut self, key: PageNo) -> Option<V> {
        let mut hole = self.find(key);
        let (_, value) = self.slots[hole].take()?;
        self.len -= 1;
        // Emptying the slot could cut a run in two. A search that starts
        // before the hole would stop at the hole and miss the keys after it.
        // So walk the rest of the run, and move an entry back into the hole
        // when its search would pass the hole: when its home slot is at the
        // hole or before it.
        let mut i = self.next(hole);
        while let Some((k, _)) = self.slots[i] {
            if self.distance(self.home(k), i) >= self.distance(hole, i) {
                self.slots[hole] = self.slots[i].take();
                hole = i
            }
            i = self.next(i);
        }
        Some(value)
    }
}
