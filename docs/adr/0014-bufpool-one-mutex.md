# One mutex for the pool's shared state, a latch per frame, and no I/O under the mutex

A fetch changes the page table, the pins, the dirty flags and the policy together, and each step is a few memory operations, so one mutex protects them all. Each frame has its own `RwLock` for its bytes, and the pool never holds the mutex during a disk read. Hits do not scale across threads, because they all wait for the one mutex.
