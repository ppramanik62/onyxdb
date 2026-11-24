# Keys up to 512 bytes, values up to 1 MiB

An engine with 4 KiB pages must fit several of the longest keys in one page, so a full page can always split in two. `Batch` checks the limits when it takes a key or value, so no engine repeats the check. The caller splits bigger values.
