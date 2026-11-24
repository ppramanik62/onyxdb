# Kv::scan answers empty ranges, and engines implement scan_range

`scan` is a provided method: it returns an empty scan for a range that can hold no key, and otherwise calls the engine's `scan_range`. Every engine used to repeat that check, and one that forgot it could panic on a backward range. An engine that overrides `scan` itself would lose the check, so engines implement only `scan_range`.
