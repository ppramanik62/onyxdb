# The LSM tree uses leveled compaction and Bloom filters

Writes go to the log, then to a memtable, which is flushed to level 0, where tables may overlap. From level 1 on, tables in a level do not overlap, and each level is 10 times bigger than the one before. Each table has a Bloom filter so a read skips tables that cannot hold its key, and a delete is a tombstone until compaction reaches the bottom level.
