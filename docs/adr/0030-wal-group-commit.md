# Group commit: writers that wait at the same moment share one sync

`flush_to` lets every writer that waits for a sync at that moment share the next one, so commits per second grow with the number of writers instead of staying at one sync each. This replaces the one sync per commit of ADR 0021. Each writer waits up to one extra sync.
