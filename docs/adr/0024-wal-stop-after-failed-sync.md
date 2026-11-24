# After a failed write or sync, the log stops until it is opened again

On Linux a failed `fsync` drops the unsaved writes, and the next `fsync` succeeds, so a retry would report lost data as saved. Every call returns `Stopped` until the caller opens the log again, which reads back what is really on the disk. `kv` engines make the same promise.
