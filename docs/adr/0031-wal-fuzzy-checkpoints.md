# Fuzzy checkpoints, and the log deletes whole segments before them

A checkpoint saves the position where replay can start, without stopping writers, and then the log deletes the whole segments before that position. Replay then reads only what came after the last checkpoint. Records just before the position may still be replayed, and the engine skips them.
