# The client retries only statements that cannot have committed, and the shell runs on a Session trait

The client library retries a statement only when it cannot have committed. The shell runs on a `Session` trait that `client` defines, with a network session in `client` and an embedded one in `onyxdb`, so the same shell works with or without a server.
