# The log uses LevelDB's format: 32 KiB blocks and checksummed fragments

Records are cut into fragments inside 32 KiB blocks, and each fragment has a CRC-32C over its length, its kind and its data. Block boundaries come from arithmetic, so damage cannot move them, and a torn length field is caught. It costs 7 bytes per fragment and up to 6 bytes of trailer per block.
