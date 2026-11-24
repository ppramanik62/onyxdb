# Statements take intention locks on their tables, and DDL takes X

A statement takes IS or IX locks on the tables it uses, and DDL takes an X lock, so `DROP TABLE` waits for the queries that are running on the table.
