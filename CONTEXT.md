# onyxdb

A SQL database built from scratch to learn how databases work inside. The terms below are the ones the built layers use: files, pages, the buffer pool, the log and the key-value interface.

## Language

### Files and crashes

**Crash**:
A power cut that `vfs::Mem` simulates: synced data stays, and unsynced writes are kept, lost or torn, as a seed decides.
_Avoid_: failure, kill

**Torn write**:
A write that a crash left in part: some sectors new, some old.
_Avoid_: partial write

### Pages

**Page**:
A fixed-size piece of a page file, found by its page number, with a kind, an LSN and a checksum.
_Avoid_: block (a block belongs to the log), sector

**Header**:
Page 0 of a page file, which describes the whole file.
_Avoid_: superblock, meta page

**Slotted page**:
A page that holds many items of different sizes, with slots at the front that point at items packed at the back.

**Item**:
One run of bytes in a slotted page, found through its slot.
_Avoid_: cell, tuple, record (a record belongs to the log)

**Slot**:
The entry at the front of a slotted page that points at one item. Slot order is item order.

**Overflow chain**:
A value too big for one page, split across linked overflow pages.

**Freelist**:
The linked list of freed pages, kept in the free pages themselves, reused before the file grows.

**Page store**:
Anything that reads and writes pages by number: the page file, or the buffer pool in front of it.

### Buffer pool

**Frame**:
One page-sized slot of memory in the buffer pool.
_Avoid_: buffer, cache slot

**Pin**:
A count on a frame that keeps its page from being evicted while code uses it.

**Latch**:
A short reader-writer lock on a frame's bytes, held while code reads or changes the page.
_Avoid_: lock (a lock protects data a transaction uses, until commit)

**Dirty page**:
A page in a frame whose bytes differ from the page in the file.

**Eviction policy**:
The rule that picks which unpinned frame to reuse: CLOCK or LRU-K.

**Write-ahead rule**:
A dirty page may reach the file only after the log is durable up to the page's LSN.

### The log

**Log**:
The append-only stream of records in segment files.
_Avoid_: journal

**Record**:
One entry appended to the log, cut into fragments when it spans blocks.
_Avoid_: entry, message

**Fragment**:
The part of a record that lies inside one block.

**Block**:
A fixed-size unit of the log. A reader can always start at a block boundary.
_Avoid_: page

**Segment**:
One log file, named by the LSN where it starts.

**LSN**:
A log sequence number: the byte position just past a record. 0 means "never logged".

**Replay**:
Reading every durable record back after an open, oldest first.
_Avoid_: recovery (recovery is what an engine does with the replayed records)

**Stopped**:
The state of a log or engine after a failed write or sync: every call fails until it is opened again.

### Key-value storage

**Batch**:
A list of puts and deletes applied as one: the only way to write to an engine.
_Avoid_: transaction, write set

**Engine**:
A storage structure behind the `Kv` interface.
_Avoid_: backend, store

**Model engine**:
The engine that keeps a `BTreeMap` in memory and its batches in the log. It is the reference the other engines are checked against.

**Prefix**:
The first k applied batches, for some k. After a crash, an engine holds exactly one prefix.

**Suite**:
The shared tests in `kv::suite` that every engine must pass.

**Step**:
One operation of a suite test: a get, a scan, a batch, or opening the engine again.

### Planned layers

These terms come from the ADRs for the layers that are not built yet.

**Checkpoint**:
A saved log position where replay can start. The segments before it can be deleted.

**Group commit**:
One log sync shared by every writer that waits at the same moment.

**Memtable**:
The sorted, in-memory part of an LSM tree that takes new writes.

**SSTable**:
A sorted, immutable file of an LSM tree.
_Avoid_: segment (a segment belongs to the log)

**Tombstone**:
A record in an LSM tree that says a key was deleted.

**Snapshot**:
The point in commit order that a transaction reads at: it sees every version committed before it, and none after.

**Commit timestamp**:
The number a transaction's versions carry, from a counter saved to disk.

**Version**:
One value of a key, written by one transaction.

**Intent**:
A version that its transaction has not committed yet.

**Transaction record**:
The record that says whether a transaction with intents committed.

**Lock**:
A claim on a table, row or range that a transaction holds until it commits.
_Avoid_: latch (a latch protects bytes in memory for a short time)

**Session**:
One client connection and its transaction state.

### The project

**Milestone**:
A version of onyxdb that runs, from M0 to M6. Each one builds only the parts of each crate it needs.
