# A commit is one batch, visible only after it is applied

A transaction commits as one atomic batch at its commit timestamp, and new snapshots see it only after the batch is applied, because the `kv` contract lets a reader in another thread see part of a batch while it is applied. Timestamps come from a counter saved to disk, so a restart never reuses one.
