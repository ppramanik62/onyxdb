# One mutex for the log, held during sync, and no buffer of its own

Records go straight into the page cache with `write_at`, and one mutex protects the whole state, sync included. This is simple and correct, but every commit waits for a sync of its own: about 2,000 commits per second, and 8 threads are no faster than 1.
