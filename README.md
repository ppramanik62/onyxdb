# onyxdb

A SQL database written from scratch in Rust, with the standard library only. It
has SQLite-like SQL, crash-safe storage, transactions, and many clients over its
own protocol.

The references behind each layer are in [engineering-notes](https://github.com/PPRAMANIK62/engineering-notes).

## Crates

Each crate has a README with its design, its format and the decisions behind it.
[ARCHITECTURE.md](ARCHITECTURE.md) shows how the crates fit together.

| Crate | What it is | State |
|---|---|---|
| [`vfs`](crates/vfs) | file I/O behind a trait: the real disk, or a file system in memory that can crash, tear writes and fail `fsync`. Also file locks | done |
| [`pagefile`](crates/pagefile) | a file of fixed-size, checksummed pages: slotted pages, a freelist and overflow chains | done |
| [`bufpool`](crates/bufpool) | a buffer pool: frames, eviction, the write-ahead rule, latches | done |
| [`wal`](crates/wal) | a write-ahead log: records in blocks, segments, sync, replay, dropping old segments. Group commit and checkpoints come later | part 1 done |
| `kv` | the ordered key-value trait, a model engine on `BTreeMap` and the log, and a shared test suite | in progress |
| `btree` | a B+tree engine, crash-safe through page images in the log | planned |
| `lsm` | a log-structured merge tree engine | planned |
| `mvcc` | transactions: snapshots, a lock manager, deadlocks, serializable isolation with phantoms | planned |
| `sql` | parser, catalog, schema locks, prepared statements, planner and executor | planned |
| `wire` | the client-server protocol | planned |
| `server` | sessions over TCP, many clients at the same time | planned |
| `client` | a client library and a REPL | planned |
| `onyxdb` | the binary: owns the data directory, recovers and serves clients. Also integrity checks and backups | planned |

The database is built in milestones, and each milestone is a version that runs.
[ARCHITECTURE.md](ARCHITECTURE.md#milestones) lists them.

| Milestone | What works | State |
|---|---|---|
| M0 | reading and writing cached, checksummed pages | done |
| M1 `v0.1` | put, get, delete and scan keys in a store that survives `kill -9` | in progress |
| M2 `v0.2` | SQL in a shell | planned |
| M3 `v0.3` | connecting over the network | planned |
| M4 `v0.4` | many clients at the same time | planned |
| M5 `v0.5` | indexes, joins and optimized queries | planned |
| M6 `v1.0` | a choice of two engines, with all of it proven | planned |

## Build and test

```text
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
cargo doc --open
```

`rust-toolchain.toml` pins the compiler version. CI runs the same checks on
every push.

## Rules

- Standard library only. A crate depends only on the crates below it.
- No `unsafe`, and all file I/O goes through `vfs`.
- Dependencies point down. `sql` can use `mvcc`, but `mvcc` never uses `sql`.
  When a lower crate needs something from a crate above it, the lower crate
  defines a trait and the crate above implements it.
- Each crate's README describes the code as it is now, and it changes in the
  same commit as the code.

## License

[MIT](LICENSE).
