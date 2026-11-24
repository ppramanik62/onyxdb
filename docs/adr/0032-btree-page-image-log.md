# The B-tree logs one record of full page images per batch

Each batch changes pages in the buffer pool, then appends one log record that holds a full image of every page it changed, the header and freelist pages included. A record is whole or missing after a crash, so the record is the commit, and recovery writes the images back into the page file, which also repairs pages and headers that a crash tore. The B-tree needs no undo, and a batch costs a full page in the log for every page it changes.
