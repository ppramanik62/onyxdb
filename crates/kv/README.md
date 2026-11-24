# kv

One ordered key-value interface for every storage engine.

```text
        get(key) · scan(from, to) · apply(batch, sync)
                         │
   ┌─────────────────────▼─────────────────────┐
   │ trait Kv                                  │
   └─────────────────────┬─────────────────────┘
                  any storage engine
```

The code above storage needs four things from an engine: read a key, read a
range of keys in order, write a group of changes all together, and know when
those changes survive a crash. `Kv` is the trait with exactly those operations.
Every engine implements it, so the code above can run on any of them.

The trait is small, and the promises behind it are the important part:
**a batch is applied whole or not at all, and after a crash the engine holds
some prefix of the applied batches, which includes every batch applied with
`sync`.**

| Term | Meaning |
|---|---|
| key, value | byte strings. Keys are ordered by comparing their bytes |
| batch | a list of puts and deletes that an engine applies whole or not at all |
| engine | a type that implements `Kv` |
| applied | `apply` returned `Ok`. Every later read sees the batch |
| synced | applied with `sync` set. The batch survives a crash |

## The public API

| Item | What it is | When to use it |
|---|---|---|
| `Kv` | the trait: `get`, `scan`, `apply` | to read and write any engine, or to write a new engine |
| `Batch` | a list of ops: `new`, `put`, `delete`, `ops`, `len`, `is_empty`, `encode`, `decode` | the only way to write. A single put is a batch with one op |
| `Op` | `Put(key, value)` or `Delete(key)` | an engine reads them from `Batch::ops` |
| `Iter` | the boxed iterator that `scan` returns, of `(key, value)` pairs | to read a range in order |
| `empty_range` | true when a pair of bounds holds no keys | an engine calls it at the start of `scan` |
| `MAX_KEY`, `MAX_VALUE` | 512 bytes and 1 MiB | the limits a batch checks |
| `Error`, `Result` | `KeyTooLong`, `ValueTooLong`, `Vfs`, `Corrupt`, `Stopped` | every call that can fail returns them |
| `Model` | the model engine: a `BTreeMap` in memory and a `wal` log of batches. `create`, `open` | an engine to trust, for tests of the code above storage |
| `suite::Engine` | how the suite creates and opens an engine on a `vfs::Mem` | to run the suite on a new engine |
| `suite::Step` | one step of a test: get, scan, scan with writes during it, batch, reopen | to write or read a test by hand |
| `suite::generate`, `run`, `threads` | random steps from a seed; run steps against a `BTreeMap`; writers in many threads | to check an engine against the contract |
| `suite::shrink`, `check`, `Failure` | cut a failing test down to the steps that matter; run many seeds and shrink the first failure | to read a failure |
| `suite::crashes`, `check_crashes` | crash the engine after every change to its files, and fail every sync once | to check the crash promise |

## Use

```rust
use kv::{Batch, Error, MAX_KEY, Op};

// Ops run in the order they were added, so "b" ends up deleted.
let mut batch = Batch::new();
batch.put("a", "1").unwrap();
batch.put("b", "2").unwrap();
batch.delete("b").unwrap();
assert_eq!(batch.len(), 3);
assert_eq!(batch.ops()[0], Op::Put(b"a".to_vec(), b"1".to_vec()));

// The batch refuses a key over the limit when it takes the key.
let err = batch.put(vec![0; MAX_KEY + 1], "x").unwrap_err();
assert!(matches!(err, Error::KeyTooLong { .. }));

// An engine that logs its batches stores these bytes.
let bytes = batch.encode();
assert_eq!(Batch::decode(&bytes).unwrap(), batch);
```

## The contract

| Promise | Exactly |
|---|---|
| order | keys compare byte by byte, and a key that is a prefix of another comes first. A scan returns keys in strictly increasing order, inside its bounds |
| empty values | an empty value is a value: `get` returns `Some` with no bytes, not `None` |
| empty ranges | a scan whose start is after its end, or equals its end with either bound excluded, returns nothing |
| visible | once `apply` returns `Ok`, every later `get` and `scan` sees the whole batch |
| reopen | with no crash, closing and opening the engine keeps every applied batch, synced or not |
| crash | after a crash, the engine holds exactly some prefix of the applied batches, in order, including every synced batch. No batch survives in part |
| open | what an engine holds just after it opens survives a later crash |
| errors | if `apply` fails, its batch survives whole or not at all. The engine may return `Stopped` for every later write until it is opened again |
| scans during writes | no snapshot. A scan sees each key as it is when it reaches it. A key that a batch changes during the scan may come back old, new, or not at all |
| threads | batches are applied one after another. A reader may see part of a batch, but never part of one value |

## Format

An engine that keeps a log stores each batch as the bytes from
`Batch::encode`: the ops one after another, with no count in front. The length
of the log record ends the batch.

```text
put     0      1        3          3+k       7+k
        ┌──────┬────────┬──────────┬─────────┬───────────┐
        │ 1    │ k: u16 │ key (k)  │ v: u32  │ value (v) │
        └──────┴────────┴──────────┴─────────┴───────────┘
delete  0      1        3          3+k
        ┌──────┬────────┬──────────┐
        │ 2    │ k: u16 │ key (k)  │         numbers are little-endian
        └──────┴────────┴──────────┘
```

## Decisions

- **Writes go only through a batch.**
  Why: "whole or not at all" then has one meaning for every write, and an engine
  has one write path to make crash-safe.
  Cost: a single put needs a `Batch` around it.
- **Keys up to 512 bytes, values up to 1 MiB, checked when the batch takes them.**
  Why: an engine with 4 KiB pages must fit several of the longest keys in one
  page, so a full page can always split in two. Checking in the batch means no
  engine repeats the check.
  Cost: bigger values have to be split by the caller.
- **The encoding has no op count.**
  Why: the log record already says how long the batch is.
  Cost: a batch can only be decoded from bytes whose end is known.
- **Scans see no snapshot.**
  Why: an engine that changes pages in place can keep this promise without
  copying pages or blocking writers.
  Cost: code that needs a consistent view of many keys builds it above this
  trait.
- **After a failed write, an engine may stop until it is opened again.**
  Why: the same reason as in `wal`: a retry after a failed `fsync` can report
  lost data as saved.
  Cost: one failed write stops all writes.

## Known gaps

- `Model` never removes old log records, so opening it replays every batch it
  ever applied. Fine for tests, wrong for real data.
- The crash checks are slow: every change to the files is one power cut, crashed
  with several seeds, so tests use short step lists.
- `lib.rs` keeps its own docs, so this README is not yet the crate's front page
  in `cargo doc`, and the example above does not run as a doctest.

## Tests

```text
cargo test -p kv
```

| Test | What it checks |
|---|---|
| `tests/batch.rs` | encode and decode round-trip on random batches, bad bytes give `Corrupt` with the right message, the limits are exact, empty ranges, and random bytes never make `decode` panic |
| `tests/model.rs` | each promise of the contract on `Model`, one test per promise |
| `tests/suite.rs` | the model passes random steps and threads, and the generator makes every kind of step, empty and backward scans included |
| `tests/shrink.rs` | an engine whose scan skips its last key fails, and its failure shrinks to a few steps |
| `tests/crash.rs` | the model survives a crash at every write; an engine that applies a batch in parts passes without crashes and fails with them |

`cargo run -p kv --example order` prints 1,000 random keys in the model's scan
order, so the order can be compared with `sqlite3` sorting the same BLOBs.

The references behind this crate are in [engineering-notes](https://github.com/PPRAMANIK62/engineering-notes/blob/main/database-references/README.md#key-value-storage).
