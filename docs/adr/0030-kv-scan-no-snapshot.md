# A scan sees no snapshot

A scan sees each key as it is when the scan reaches it, so an engine that changes pages in place keeps the promise without copying pages or blocking writers. Code that needs a consistent view of many keys builds it above `kv`.
