# The MANIFEST is replaced with write, sync, rename and directory sync

The MANIFEST lists the live tables. The engine writes a new one, syncs it, renames it over the old one and syncs the directory, so a crash in the middle of a compaction leaves either the old set of tables or the new one, never a mix.
