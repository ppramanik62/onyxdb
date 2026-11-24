# The page size is fixed when a file is created: a power of two from 512 to 32768

Offsets inside a page are `u16`, and a `u16` cannot hold 65536. Every page in a file has the same size, and 64 KiB pages are not possible.
