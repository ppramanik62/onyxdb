# The standard library only, and no unsafe code

onyxdb exists to learn how a database works inside, so every layer is written here: formats, data structures, checksums, the hash table of the buffer pool. No crate depends on anything outside the workspace and the standard library, and no crate uses `unsafe`. This costs speed and time: we write code that a library would give us, such as a portable CRC-32C instead of the CPU instruction.
