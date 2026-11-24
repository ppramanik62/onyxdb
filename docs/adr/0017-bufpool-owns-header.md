# The pool keeps its own header, and flush writes it last

Allocating a page changes the header in memory only, and the pool implements `PageStore` itself, so the freelist and overflow chains go through the cache. `flush` writes the pages, syncs, then writes the header and syncs again, so the header on disk never counts a page the file does not have. A crash while the header is written can tear page 0.
