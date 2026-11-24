# wal

A write-ahead log: an append-only stream of records that survives a crash.

```text
 append(record) ──▶ LSN            written to the file, not durable yet
 flush_to(LSN)                     synced: the record is durable
 ... crash ...
 open, then replay ──▶ records     every durable record, oldest first
```

A database changes pages in memory and writes them to its data files later.
Before it reports a change as saved, it appends a record of the change to the
log and syncs the log. After a crash, it replays the log and applies the records
again. The log is a good place for this sync, because it writes at the end of
one file, and that is the fastest write a disk can do.

The log stores records as bytes and never reads inside them. It promises one
thing: **after a crash, replay returns each record whole or not at all, in the
order the records were appended, and no durable record is missing.**

| Term | Meaning |
|---|---|
| record | the bytes a caller appends |
| LSN | log sequence number: a position in the log, in bytes. A record's LSN is the position just past its last byte |
| durable | on disk, so a crash cannot remove it |
| segment | one file of the log. The log is stored in a run of segments |
| replay | reading every durable record back, oldest first |

## The public API

| Item | What it is | When to use it |
|---|---|---|
| `Wal` | the log: `create`, `open`, `append`, `flush_to`, `end`, `durable`, `replay`, `drop_before` | to log changes and read them back after a crash |
| `Lsn` | a `u64` position in the log. 0 means "never logged" | `append` returns one for each record |
| `Replay`, `Record` | an iterator over the durable records, and one record: its `lsn` and its `data` | `Wal::replay` returns them |
| `Error`, `Result` | `Vfs`, `Stopped`, `Corrupt` | every call that can fail returns them |
| `format` | `encode`, `decode`, `Kind`, `Fragment`, `BLOCK`, `HEADER`: the on-disk format | to read or write the format directly, as the examples do |
| `segment` | `path`, `start`, `list`: segment file names | to find a log's segment files |
| `crc::crc32c` | the CRC-32C checksum | to checksum bytes the same way fragments are checksummed |

## Use

```rust
use std::sync::Arc;
use vfs::Mem;
use wal::{BLOCK, Wal};

let mem = Mem::new();
let size = 4 * BLOCK as u64;
let wal = Wal::create(Arc::new(mem.clone()), "wal", size).unwrap();

// Append two records and make them durable.
let apple = wal.append(b"put apple 1").unwrap();
let banana = wal.append(b"put banana 2").unwrap();
wal.flush_to(banana).unwrap();

// This record is written but not synced, so a crash may lose it.
wal.append(b"put cherry 3").unwrap();
mem.crash(0);

// After the crash, open the log and replay it.
let wal = Wal::open(Arc::new(mem), "wal", size).unwrap();
let records: Vec<_> = wal.replay().unwrap().map(|r| r.unwrap()).collect();
assert_eq!(records[0].data, b"put apple 1");
assert_eq!(records[0].lsn, apple);
assert_eq!(records[1].lsn, banana);

// Once the change in the first record is safe in the data files, the
// segments that hold only older records can be deleted.
wal.drop_before(apple).unwrap();
```

## Format

The log is one stream of bytes, divided into blocks of 32 KiB. A record is
written as one or more fragments, and a fragment never crosses a block boundary.

```text
a fragment
0         4      6      7
┌─────────┬──────┬──────┬─────────────────┐
│ CRC-32C │ len  │ kind │ data: len bytes │    numbers are little-endian
└─────────┴──────┴──────┴─────────────────┘
          └───── the CRC covers these ────┘

a block of 32 KiB
┌────────────┬──────────────┬───────────────────────┬─────────┐
│ fragment   │ fragment     │ fragment              │ trailer │
└────────────┴──────────────┴───────────────────────┴─────────┘
```

| Kind | Byte | What the fragment holds |
|---|---|---|
| full | 1 | a whole record |
| first | 2 | the start of a record that continues in the next block |
| middle | 3 | a part from inside such a record |
| last | 4 | the end of such a record |

When fewer than 7 bytes are left at the end of a block, a header does not fit
there. Those bytes stay zero, as the block's **trailer**, and the next fragment
starts in the next block.

The stream is stored in segment files. Each segment holds a whole number of
blocks, and its name is the LSN where it starts, as 16 hex digits:

```text
wal/0000000000000000.wal   0000000000020000.wal   0000000000040000.wal
    LSN 0 to 131071            131072 to 262143        262144 to the end
```

`examples/layout.rs` prints the fragments of LevelDB's example records:

```text
$ cargo run --release --example layout
record  bytes      LSN
A        1000     1007
B       97270    98298
C        8000   106311

    at  block  offset  kind     data
     0      0       0  full      1000
  1007      0    1007  first    31754
 32768      1       0  middle   32761
 65536      2       0  last     32755
 98298      2   32762  trailer  6 zeros
 98304      3       0  full      8000
```

## Opening after a crash

```text
open:   list the segments, check their names
        read every record, stop at the first bad fragment   ──▶ the valid end
        delete later segments, sync the directory
        shorten the last segment to the valid end
        write the rest of that segment again, and sync it
replay: every record up to `durable`, oldest first
```

After `open`, every record in the log is durable, and `end` and `durable` are
both the LSN of the last whole record.

## Known gaps

- Every commit has its own `fsync`, so threads that commit at the same time
  wait for the disk one after another. Group commit would let them share one
  sync.
- A flipped bit in synced data ends the log silently, because replay stops at
  the first bad fragment.
- There are no checkpoints yet. The caller decides when to call `drop_before`.
- Calling `drop_before` while a `Replay` iterator runs can delete a segment
  that the iterator has not read yet. The iterator then returns an error.

## Tests and benchmarks

```text
cargo test -p wal
cargo bench -p wal
cargo run --release -p wal --example layout   the fragments of LevelDB's example
cargo run --release -p wal --example torn     100 crashes of the same log
cargo run --release -p wal --example hole     a failed sync, in a plain file and in the log
```

| Test | What it checks |
|---|---|
| `tests/format.rs` | LevelDB's example to the byte, records near the end of a block, an empty record, and every single-bit flip in a fragment |
| `tests/append.rs` | LSNs are positions, segments fill up and are named by their start, `create` refuses an existing log, and a failed write or sync stops the log |
| `tests/replay.rs` | records and LSNs after `open`, only durable records come back, a torn last record and every record after a tear are cut off, later segments are removed, files that break the rules are `Corrupt`, and 200 random damages never panic |
| `tests/drop.rs` | whole segments are dropped, replay skips the rest of a dropped record, and 50 seeds of random appends, flushes, drops and reopens against a model |
| `tests/crash.rs` | a crash at every write, and a failed sync at each flush followed by a crash at every later write: every acknowledged record comes back |

`benches/wal.rs` prints records appended per second without a sync, and commits
per second with 1 and 8 threads. It runs on the real disk under `target/`. The
first run is saved in `benches/baseline.txt`:

```text
append, not synced   1 thread      866803 records/s
commit               1 thread        1960 commits/s
commit               8 threads       2046 commits/s
```

The references behind this crate are in [engineering-notes](https://github.com/PPRAMANIK62/engineering-notes/blob/main/database-references/README.md#write-ahead-log).
