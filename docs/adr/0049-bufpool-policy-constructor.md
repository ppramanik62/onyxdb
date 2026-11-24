# The pool builds its policy from its own frame count

`BufPool::new` takes a function that makes the policy, such as `Clock::new`, and calls it with the pool's frame count. A policy made for a different number of frames was a mistake that nothing caught; now it cannot be written. A policy chosen while the program runs is passed as a function that returns a `Box<dyn Policy>`.
