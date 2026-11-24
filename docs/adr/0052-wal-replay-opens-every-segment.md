# Replay opens every segment when it starts

An open file keeps working after it is removed, so a replay that holds every segment open cannot lose one to `drop_before`. Before, a `drop_before` during a replay could delete a segment the replay had not reached, and the replay failed. A replay holds one open file per segment, which checkpoints keep few.
