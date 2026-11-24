# A model engine and one shared suite decide if an engine is correct

`Model` keeps a `BTreeMap` in memory and logs its batches in `wal`. The suite in `kv::suite` runs random steps on an engine and on a `BTreeMap`, shrinks a failure to the steps that matter, and crashes the engine after every change to its files. Every engine must pass it, and the crash checks are slow.
