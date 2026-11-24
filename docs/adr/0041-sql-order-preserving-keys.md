# Tables, indexes and the catalog are keys whose byte order matches value order

A row is `table_id | primary key`, an index entry is `index_id | indexed columns | primary key` with an empty value, and the catalog is `0 | "table" | name`; a table with no primary key gets a hidden row id. The key encoding keeps value order as byte order, so a range in SQL becomes one range scan in `kv`.
