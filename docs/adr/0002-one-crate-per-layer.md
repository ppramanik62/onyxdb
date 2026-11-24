# One crate per layer, and dependencies point down

Each layer is its own crate in one Cargo workspace, and a crate uses only the crates below it. When a lower crate needs something from a crate above it, the lower crate defines a trait and the crate above implements it, as `bufpool` does with `LogHook`. A change to a lower crate fixes every crate above it in the same commit.
