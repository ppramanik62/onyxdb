# wal copies the CRC-32C code instead of using pagefile

The log does not need pages, so it depends only on `vfs`. There are two copies of 30 lines.
