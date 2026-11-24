# Numbers on disk are little-endian

x86 and ARM are little-endian, so encoding a number is a plain byte copy. SQLite chose big-endian, so hex dumps of our files show numbers with their bytes in reverse order.
