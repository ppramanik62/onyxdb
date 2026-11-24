# An 8-byte LSN in every page except page 0

The log needs to know which change each page saw last, for the write-ahead rule and for recovery. `pagefile` stores the number after the kind byte and never reads it. It costs 8 bytes per page.
