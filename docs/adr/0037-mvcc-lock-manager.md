# One lock manager, with locks held until commit

Tables take intention locks (IS, IX, S, X), rows take S or X locks, and ranges take S or X locks so phantoms are locked too. Waits form a wait-for graph, and a cycle is a deadlock in which the youngest transaction aborts.
