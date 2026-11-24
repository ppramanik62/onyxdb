# Every write creates a version, keyed by user key and inverted commit timestamp

A version's key is the user key followed by its commit timestamp with the bits inverted, so newer versions sort first and a reader finds the newest version older than its snapshot with one seek. Garbage collection removes versions older than the oldest running snapshot.
