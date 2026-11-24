# One session per connection, on a fixed thread pool

Each connection has one session, which holds that connection's transaction state. A thread pool of fixed size refuses work it cannot take, and shutdown rolls back open transactions and flushes.
