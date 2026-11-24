# A full segment and its directory are synced before the next segment is created

Otherwise a crash could keep records in the new segment and lose earlier ones in the old segment, and replay would stop at the gap and lose records the log reported as saved. It costs two syncs each time a segment fills.
