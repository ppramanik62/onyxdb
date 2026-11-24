# vfs

File I/O behind a trait. The database runs on `Disk`, and its tests run on
`Mem`, a file system in memory that can crash.

```text
                 database code
                       │
            Vfs and File (traits)
              ┌────────┴────────┐
            Disk               Mem
        the real disk     in memory, for tests:
                          cut the power, crash, inject faults
```

No crate above this one uses `std::fs`. All file I/O goes through these traits,
so the tests can crash every crate that writes to disk at every write.

## The public API

| Item | What it is | When to use it |
|---|---|---|
| `Vfs` | a file system: `open`, `exists`, `remove`, `rename`, `create_dir`, `list`, `sync_dir` | to work with files or directories |
| `File` | an open file: `read_at`, `write_at`, `len`, `set_len`, `sync`, `try_lock` | to read or write a file |
| `Open` | `Existing` or `CreateNew` | to open a file |
| `Disk` | the real file system, below a root directory | to run the database |
| `Mem` | a file system in memory that can crash | to test code that writes files |
| `Mem::changes()` | the number of changes made so far | to know where to cut the power |
| `Mem::power_off_after(n)` | allows `n` more changes, then fails every change | to cut the power at a chosen point |
| `Mem::crash(seed)` | drops some unsynced data, chosen by the seed | to see what a crash leaves |
| `Mem::fail_next(Fault)` | makes the next write or sync fail: `WriteError` writes nothing, `ShortWrite` writes half, `SyncError` loses the unsynced writes | to test how code handles a disk error |
| `crash_test` | crashes a workload after every change, with many seeds, and returns a `Report` | to check a whole workload against every crash point |
| `Report`, `Failure` | `runs` and `failures`. Each failure has `changes`, `seed` and `message` | to read the result of a crash test |
| `Rng` | a small seeded random generator | for random test input that is the same on every run |
| `Error`, `Result` | every error, with the operation and the path | every call returns them |

Paths are relative to the root, like `data/pages.pf`. The empty path is the root
itself.

## Writing a file safely

```text
write_at   the bytes are in memory      a crash can keep, lose or tear them
sync       the bytes are on disk        safe
sync_dir   the name is on disk          safe (needed after create, rename, remove)
```

```rust
use std::path::Path;
use vfs::{Disk, Open, Vfs};

let root = std::env::temp_dir().join(format!("vfs-readme-{}", std::process::id()));
std::fs::create_dir_all(&root).unwrap();
let disk = Disk::new(&root);

let file = disk.open(Path::new("notes"), Open::CreateNew).unwrap();
file.write_at(b"hello", 0).unwrap();
file.sync().unwrap();
disk.sync_dir(Path::new("")).unwrap();

std::fs::remove_dir_all(&root).unwrap();
```

With `Mem::new()` in place of `Disk::new(&root)`, the rest of the code is the
same.

Do not retry a `sync` that failed. On Linux, a failed sync drops the writes it
could not save, and the next sync then succeeds with nothing to write. So after
a failed sync, stop, and recover from what is on the disk. `Mem` behaves the
same way.

## Testing against crashes

```text
crash_test(seeds, workload, check) -> Report

  workload: Fn(&Mem, &mut A) -> Result<()>          does the work, and records in A what it was told is safe
  check:    Fn(&Mem, &A) -> Result<(), String>      after a crash, checks that all of that is still there

  run the workload once, and count its changes: 1, 2, 3 ... N

  for each n from 0 to N, and each seed:
    new Mem ──> power off after n ──> workload ──> crash(seed) ──> check
                                                                     │
                                          each failure keeps (n, seed)
```

`A` is any type with a `Default`, like a `bool` or a count of saves. With it,
the check can tell data that was promised and lost from data that was never
promised.

The `kvfile` example saves a small file in two ways and crash-tests both:

```text
in place: 240 crashes, 34 failed
first: power off after 6 changes, seed 5: store holds "apple=1\nbanana", which is no version at all
by rename: 320 crashes, 0 failed
```

To run a failure again, run the workload on a new `Mem` after
`power_off_after(n)`, then call `crash(seed)`.

## What a crash keeps

| What | After `crash(seed)` |
|---|---|
| synced bytes and names | always kept |
| unsynced writes | each 512-byte sector is kept, lost, or torn |
| unsynced `set_len` | kept or lost |
| unsynced name changes | kept in order, up to some point |
| writes from a failed `sync` | lost |
| locks | released |
| handles opened before the crash | return `Error::Stale` |

Not supported: memory-mapped files, shared locks, Windows, and async I/O.

## Tests and examples

```text
cargo test -p vfs                       Disk and Mem, crashes, faults, crash_test
cargo run -p vfs --example kvfile       crash-test two ways of saving a file
cargo run -p vfs --example fsyncgate    what a failed sync does
cargo run -p vfs --example replace      why sync_dir matters
cargo run -p vfs --example torn         a torn sector, seed by seed
```

The references behind this crate are in [engineering-notes](https://github.com/PPRAMANIK62/engineering-notes/blob/main/database-references/README.md#virtual-file-system).
