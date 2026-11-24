# Snapshot isolation, and serializable isolation through SSI

Under snapshot isolation, reads use the snapshot and each write X-locks its row; if the row changed since the snapshot, the writer aborts. Serializable adds SSI: every key and range read is tracked, and a dangerous read-write cycle aborts one transaction. `SELECT ... FOR UPDATE` takes row and range locks instead.
