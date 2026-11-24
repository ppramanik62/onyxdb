# A CRC-32C in the last 4 bytes of every page

Every page, the header included, ends with a CRC-32C, so every read catches flipped bits and torn writes. It costs 4 bytes per page, and it cannot catch a lost write, because the old page still has a valid checksum.
