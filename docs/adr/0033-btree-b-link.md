# B-link right pointers for B-tree concurrency

Every node links to its right sibling. A reader that reaches a node in the middle of a split follows the right link instead of starting again from the root, so readers never wait for a split to finish.
