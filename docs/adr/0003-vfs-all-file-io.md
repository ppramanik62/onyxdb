# All file I/O goes through vfs, with a file system in memory that can crash

No crate above `vfs` touches `std::fs`. The tests replace the disk with `vfs::Mem`, which keeps synced and unsynced data apart, tears writes per 512-byte sector, fails `fsync` on request, and crashes from a seed, so the same seed gives the same failure every time. This lets `crash_test` cut the power after every change a workload makes, in every crate that writes to disk.
