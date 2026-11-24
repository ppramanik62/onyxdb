# PageStore is a trait, and the freelist and overflow chains are functions over it

A page cache implements the same trait as the file, so the freelist and overflow chains change pages and the header through the cache, like any other change. The trait has its own error type (`type Error: From<pagefile::Error>`), so the cache's own errors, such as having no free frame, reach the caller unchanged. The trait must stay stable, and a trait object must name its error.
