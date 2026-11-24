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

## Known gaps

- Changes to several pages are not atomic. A crash between two writes leaves
  some old pages and some new ones. A crash can also tear the header, and `open`
  then refuses the file. The crash example shows how often this happens.
- A lost write is not detected, because the old page still has a valid
  checksum.
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
