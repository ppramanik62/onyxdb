# Replay stops at the first bad fragment

Records after a damaged record are newer, and returning them without it would break the order the log promises. A flipped bit inside synced data ends the log there, and nothing reports it.
