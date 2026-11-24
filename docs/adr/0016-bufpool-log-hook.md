# The write-ahead rule goes through LogHook, a trait that bufpool defines

A dirty page reaches the file only after the log is durable up to the page's LSN. The pool asks through `LogHook`, so it needs to know only that rule, not how the log works, and the code above connects `wal` to it. Without a log, `NoLog` stands in.
