# The eviction policy is a trait, with CLOCK as the default

CLOCK only adds 1 to a byte per fetch, and every fetch runs the policy under the mutex. LRU-K is the other policy, for when one scan must not empty the pool. The policy never sees pins: the pool tells it which frames it may evict, so a bug in a policy cannot evict a page in use.
