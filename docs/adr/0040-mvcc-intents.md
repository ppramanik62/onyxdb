# Large transactions write intents and a transaction record

A transaction too large to buffer writes intents, which are uncommitted versions, while it runs, and one transaction record. A reader that finds an intent checks the record, and at startup `mvcc` removes the intents of transactions that did not commit.
