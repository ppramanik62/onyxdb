# open cuts the log at its valid end and writes the rest of the last segment again

Bytes after the valid end could form an old record that the next append does not overwrite. Writing the tail again makes its pages dirty, so the sync puts them on disk even after a failed `fsync` marked them clean. `open` reads the whole log and writes up to one segment.
