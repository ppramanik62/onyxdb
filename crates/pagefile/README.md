# pagefile

A file of fixed-size, numbered pages, each with a checksum.

```text
 page 0     page 1     page 2     page 3     page 4
┌────────┬──────────┬──────────┬──────────┬──────────┐
│ header │ slotted  │ overflow │ overflow │ free     │ ...
└────────┴──────────┴──────────┴──────────┴──────────┘
  what the   small      one value too big      reused before
  file is    items      for a page            the file grows
```

Page 0 is a header that says what the file is. Every other page starts with a
byte that names its kind, then the page's log sequence number (LSN). Every page
ends with a checksum, so a read catches a damaged page. All file I/O goes
through [`vfs`](../vfs), so the tests run on a file system in memory that can
crash.

## The public API

| Item | What it is | When to use it |
|---|---|---|
| `PageFile` | a page file: `create`, `open`, `header`, `page_count`, `set_user`, `sync`, and `read_page`, `write_page` for a cache in front of it | to open or create a file |
| `PageStore` | a trait: `page_size`, `read`, `write`, `read_header`, `write_header`, `new_page`, and its own `Error` type | to read or write pages, through a file or through a cache that implements the same trait |
| `Page` | the bytes of one page: `kind`, `expect_kind`, `lsn`, `set_lsn`, and methods that read and write numbers | to read or change a page's bytes |
| `PageNo`, `PageKind` | a page number, never 0, and a page's kind | to name a page or check what it holds |
| `Header` | the fields of page 0, with 32 bytes for the code that uses the crate | to read the page size, the page count or the user bytes |
| `Slotted` | many variable-size items in one page: `insert`, `get`, `remove`, `replace`, `compact` | to store small items in order |
| `freelist::allocate`, `freelist::free` | take a page, reusing a freed one first, and return a page to the freelist | to get a new page, or to free one that is no longer used |
| `overflow::write_chain`, `read_chain`, `free_chain` | store, read and free a value in a chain of pages | when a value is bigger than `slotted::max_item` |
| `check::check` | reads every page, walks the freelist, and returns a `Report` of problems | to find out whether a file is healthy |
| `crc::crc32c` | the CRC-32C checksum | to checksum bytes the same way pages are checksummed |
| `Error`, `Result` | every error, with the rule that broke and the numbers involved | every call returns them |

## Use

```rust
use pagefile::{PageFile, PageStore, Slotted, freelist, overflow};
use vfs::Mem;

let mem = Mem::new();
let mut file = PageFile::create(&mem, "db.pf", 4096).unwrap();

// A page of small items, kept in the order we insert them.
let n = freelist::allocate(&mut file).unwrap();
let mut page = file.new_page();
let mut items = Slotted::init(&mut page, 0);
items.insert(0, b"banana").unwrap();
items.insert(0, b"apple").unwrap();
file.write(n, &mut page).unwrap();

// A value too big for one page goes in a chain of overflow pages.
let big = vec![7u8; 10_000];
let first = overflow::write_chain(&mut file, &big).unwrap();
file.sync().unwrap();
drop(file);

let file = PageFile::open(&mem, "db.pf").unwrap();
let mut page = file.new_page();
file.read(n, &mut page).unwrap();
assert_eq!(Slotted::open(&page).unwrap().get(0), Some(&b"apple"[..]));
assert_eq!(overflow::read_chain(&file, first, big.len()).unwrap(), big);
```

`write` stores the page with its checksum. `read` checks the checksum, so a
damaged page comes back as an error, never as bytes.

## Format

Every number is little-endian.

```text
every page except page 0
byte 0      1          9                             size - 4    size
     +------+----------+-----------------------------+-----------+
     | kind | LSN      | laid out by the page's kind | CRC-32C   |
     +------+----------+-----------------------------+-----------+

page kinds
  0  empty     allocated, never written
  1  free      on the freelist
  2  slotted   variable-size items packed into the page
  3  overflow  one piece of a value too big for a page
```

**Page 0, the header** (no kind byte):

```text
offset  size  field
     0     8  magic: the ASCII bytes "pagefile"
     8     2  format version, 1
    10     4  page size, in bytes
    14     4  page count, page 0 included
    18     4  first page on the freelist, 0 if the freelist is empty
    22     4  number of pages on the freelist
    26    32  user bytes, for the code using this crate
    58     .  zeros
size-4     4  CRC-32C of everything before it
```

**A slotted page.** Slots grow from the front of the page, and items grow from
the back. The free space is the gap between them. Each slot is an offset and a
length, two `u16`s.

```text
0      1     9     10      12              14        16
+------+-----+-----+-------+---------------+---------+--------+-----+--------+----------+------+-----+----------+
| kind | LSN | tag | count | content start | garbage | slot 0 | ... | slot n | free gap | item | ... | CRC-32C  |
+------+-----+-----+-------+---------------+---------+--------+-----+--------+----------+------+-----+----------+
                                                                                ^ content start
```

**An overflow page.** The chain does not store the value's length. The code that
stores the pointer to the first page also stores the length.

```text
0      1     9                 13           15                   size - 4    size
+------+-----+-----------------+------------+--------------------+-----------+
| 3    | LSN | next page, or 0 | bytes used | data               | CRC-32C   |
+------+-----+-----------------+------------+--------------------+-----------+
```

**A free page.**

```text
0      1     9                      13                       size - 4    size
+------+-----+----------------------+------------------------+-----------+
| 1    | LSN | next free page, or 0 | zeros                  | CRC-32C   |
+------+-----+----------------------+------------------------+-----------+
```

With 512-byte pages, the largest slotted item is 488 bytes, and an overflow page
holds 493 bytes.

## Look inside a file

First make the sample files. The script writes them to
`crates/pagefile/samples/`, which git ignores:

```text
crates/pagefile/scripts/samples.sh
```

Then build the `pagefile` command and put it on the `PATH`, from the workspace
root:

```text
cargo build --release -p pagefile
export PATH="$PWD/target/release:$PATH"
```

Then read the samples with it:

```text
cd crates/pagefile/samples
pagefile header long.pf
pagefile pages  long.pf
pagefile page   fruit.pf 1
pagefile check  leaked.pf
```

```text
$ pagefile pages long.pf
    0  header
    1  slotted   1 item, 480 bytes of room
    2  overflow  next 3, 493 bytes
    3  overflow  next 4, 493 bytes
    4  overflow  next 0, 214 bytes

$ pagefile check leaked.pf
pages: 2 free, 1 header, 3 slotted
problem: page 4 is free but not on the freelist
```

`check` exits with 1 when it finds a problem. An item that is all printable text
prints in quotes, and any other item prints as hex.

| Sample | What it holds |
|---|---|
| `empty.pf` | only the header |
| `fruit.pf` | one slotted page holding apple, banana and cherry |
| `holes.pf` | `fruit.pf` with banana removed, which leaves a gap |
| `packed.pf` | `holes.pf` after `compact` closes the gap |
| `free.pf` | five pages, with pages 2 and 4 freed |
| `long.pf` | a 1200-byte value in a chain of three overflow pages, and a slotted page that holds its first page and length |
| `flipped.pf` | `fruit.pf` with one bit flipped, so its checksum fails |
| `leaked.pf` | a free page that is not on the freelist |
| `fruit.db` | the same three fruits stored by SQLite, if `sqlite3` is installed |

## Decisions

- **The page size is fixed when the file is created: a power of two from 512 to
  32768.**
  Why: offsets inside a page are `u16`, and a `u16` cannot hold 65536.
  PostgreSQL has the same limit.
  Cost: every page in a file has the same size, and 64 KiB pages are not
  possible.
- **Page numbers are never 0 (`PageNo(NonZeroU32)`).**
  Why: page 0 is the header, and nothing links to it. So 0 on disk can mean "no
  page", and `Option<PageNo>` stays 4 bytes.
  Cost: none so far.
- **Little-endian numbers.**
  Why: x86 and ARM are little-endian, so encoding a number is a plain byte copy.
  Cost: hex dumps show numbers with their bytes in reverse order. SQLite chose
  big-endian for the opposite reason.
- **A CRC-32C in the last 4 bytes of every page, the header included.**
  Why: it catches flipped bits and torn writes on every read. ext4 and InnoDB
  use CRC-32C, and many CPUs have an instruction for it.
  Cost: 4 bytes per page. It cannot catch a lost write, because the old page
  still has a valid checksum.
- **An 8-byte LSN after the kind byte of every page except page 0.**
  Why: the log above this crate needs to know which change each page saw last.
  This crate stores the number and never reads it.
  Cost: 8 bytes per page.
- **The checksum uses a 256-entry table built at compile time, not the CPU
  instruction.**
  Why: the crate uses std only, and one portable version is easy to check.
  Cost: slower than the hardware instruction.
- **The header keeps 32 bytes for the code that uses the crate.**
  Why: the layer above needs a place to store where its data starts, such as a
  root page, without a change to this format.
  Cost: 32 bytes that the crate never reads.
- **`sync` is a separate call, never automatic.**
  Why: the layer above knows what a commit is, so it decides when data must be
  on disk.
  Cost: a caller that forgets `sync` can lose writes in a crash.
- **All file I/O goes through `vfs`.** `create` and `open` take a `&dyn Vfs`,
  and `PageFile` holds a `Box<dyn vfs::File>`.
  Why: the tests run on `vfs::Mem`, which can tear a write or cut the power, so
  damage and crash cases are easy to test.
  Cost: one more crate below this one, and a `&dyn Vfs` in every call that opens
  a file.
- **`create` syncs the file and its directory before it returns.**
  Why: without the directory sync, a crash just after `create` can lose the
  file's name.
  Cost: two `fsync` calls on every create.
- **Reads and writes use positioned I/O (`read_at`, `write_at`).**
  Why: there is no seek before a read, so a read needs only `&self`.
  Cost: none so far.
- **`PageStore` is a trait, and `PageFile` is one implementation.**
  Why: a page cache can implement the same trait and be used in place of the
  file.
  Cost: one trait that must stay stable.
- **The freelist and overflow chains are functions over any `PageStore`, not
  methods of `PageFile`.**
  Why: with a page cache in front of the file, the pages and the header that
  these functions change also go through the cache.
  Cost: each call reads the header through the trait.
- **`PageStore` has its own error type: `type Error: From<pagefile::Error>`.**
  Why: a cache in front of the file has failures of its own, such as having no
  free frame. The freelist and overflow functions return the store's error, so
  the cache's errors reach the caller unchanged, and this crate's errors convert
  into the store's error.
  Cost: those functions return `Result<T, S::Error>`, and a trait object must
  name its error: `dyn PageStore<Error = E>`.
- **`read_page` and `write_page` read and write any page by number, with no
  check against the page count.**
  Why: a cache in front of the file keeps its own header, and that header counts
  new pages before the header on disk does. So the code that owns the header,
  the cache, does the check. The file keeps the raw page I/O and the checksum.
  Cost: code that calls them directly can write past the end. `check` reports
  those pages as extra bytes after the last page.
- **Slot order is the items' order, and inserting or removing an item moves the
  slots after it.**
  Why: the code using the page keeps its items sorted by moving 4-byte slots,
  never the item bytes.
  Cost: a slot number does not always name the same item, so nothing outside
  the page can store one. PostgreSQL keeps slot numbers fixed instead, because
  its indexes point at rows by (page, slot).
- **A removed item's bytes stay as garbage until `compact`.**
  Why: removing an item changes only the slots.
  Cost: when the free gap is too small for an insert but the garbage would make
  enough room, `insert` compacts the page first.
- **`free` puts a page at the front of the list, so the list works as a stack.**
  Why: freeing a page writes only that page and the header. Adding at the back
  would mean walking the list, or keeping a tail page that must be written
  again.
  Cost: the list is not in page order, so pages allocated together can be far
  apart in the file.
- **Taking a page off the list, and freeing a page, both write the page before
  the header.**
  Why: a crash between the two writes leaves a page that is not on the list
  (`free`), or a header that points at a page that is not free (`allocate`). In
  the first case the page is lost. In the second case the next `allocate`
  refuses the page. In neither case is the page given out twice.
  Growing the file uses the opposite order: the header first, because a store
  only writes pages below the page count. If the page write then fails, the
  count is lowered again, so a failed `allocate` leaves the header as it was.
  Cost: a crash can leak a page. `check` finds it.
- **`free` refuses a page that is already free, and `allocate` refuses a list
  page that is not free.**
  Why: a page freed twice would point at itself and be given out again and
  again.
  Cost: one kind check per call. A page leaked by a crash cannot be freed again.
- **An overflow chain does not store the value's length. The caller stores it.**
  Why: the caller already stores a pointer to the first page. With 4 more bytes
  there, `read_chain` can catch a chain that ends too early or is too long.
  Cost: a chain cannot be read without its length, and `check` cannot check
  chains.
- **Every overflow page holds at least 1 byte.**
  Why: then a chain that loops back on itself still ends after at most `len`
  pages, so `read_chain` never runs forever on a damaged file.
  Cost: none.
- **`write_chain` allocates every page of the chain before it writes any.**
  Why: each page's next page is known when the page is written, so every page
  is written once.
  Cost: a crash between the two steps leaves allocated pages that hold nothing.
- **`check` reports every problem it finds, not only the first.**
  Why: one run shows all the damage. Only an I/O error stops it.
  Cost: the caller reads a `Report` instead of a single error.
- **Errors name the rule that broke and carry the numbers involved.**
  Why: a message like "page 3 is damaged: its checksum is 1a2b3c4d, but its
  bytes give 5e6f7a8b" says what is wrong and where.
  Cost: a larger error enum.

## Known gaps

- Changes to several pages are not atomic. A crash between two writes leaves
  some old pages and some new ones. A crash can also tear the header, and `open`
  then refuses the file. The crash example shows how often this happens.
- A lost write is not detected, as the checksum decision explains.
- Every read goes to the disk. Nothing is cached.
- `check` finds a leaked free page, but nothing returns it to the freelist.
  `check` cannot find a leaked slotted or overflow page, because only the code
  that uses the crate knows which pages it still points to.
- The file never shrinks. Freed pages are reused, but the file never gets
  smaller.

## Tests and examples

```text
cargo test -p pagefile
cargo run --release -p pagefile --example crashes
```

| Test | What it checks |
|---|---|
| unit tests | page sizes, page numbers and the CRC check value |
| `tests/corrupt.rs` | refuses files of another format and files that are too short, catches a torn header, and catches every single-bit flip in a page |
| `tests/slotted.rs` | 50 seeds of 2000 random operations against a `Vec`, compaction after removals, and the largest item |
| `tests/model.rs` | 20 seeds of 400 random allocates, frees, reopens and reads against a `BTreeMap` and a `BTreeSet`, after which `check` finds no problem. Freed pages are reused before the file grows, and freeing a page twice is an error |
| `tests/overflow.rs` | values come back unchanged, freeing a chain frees every page, and a wrong length is caught |
| `tests/fuzz.rs` | damaged slotted pages, headers and whole files. Every reader returns an error and never panics |

The crash example creates a file, writes three pages, frees one and syncs. Then
it cuts the power after every change, with 20 seeds:

```text
it opens:            340 crashes, 179 failed
it opens and checks: 340 crashes, 204 failed
```

These failures are expected. Each change writes two pages, and a crash can keep
either one. Nothing in this crate makes two page writes happen together. A
write-ahead log does that, so the same workload should pass once a log protects
these writes.

The references behind this crate are in [engineering-notes](https://github.com/PPRAMANIK62/engineering-notes/blob/main/database-references/README.md#page-structure).
