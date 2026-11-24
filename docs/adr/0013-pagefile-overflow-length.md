# An overflow chain does not store its value's length

The caller stores the length next to its pointer to the first page, so `read_chain` can catch a chain that ends too early or runs too long. Every overflow page holds at least 1 byte, so a chain that loops back on itself still ends. A chain cannot be read without its length, and `check` cannot check chains.
