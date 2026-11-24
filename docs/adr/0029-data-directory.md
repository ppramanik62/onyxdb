# A data directory belongs to one process and one engine

The running process holds an exclusive lock on `data/LOCK`, so a second process cannot open the same directory. The engine, B-tree or LSM tree, is chosen when the directory is created and never changes, so each engine keeps only its own files. Moving data to the other engine means a dump and a load.
