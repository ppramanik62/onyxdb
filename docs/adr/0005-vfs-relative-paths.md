# Relative paths, and renames only inside one directory

A path is relative to the root of a `Vfs`, so it names the same file on `Disk` and in `Mem`. A rename stays inside one directory, so replacing a file needs only one `sync_dir`. A file can never move to another directory.
