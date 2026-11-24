#!/bin/sh
# Every check a change must pass before it is done. CI runs this script, and
# the pre-commit hook runs it with --quick, which leaves out the tests.
set -eu
cd "$(dirname "$0")/.."

# std only: every package in the lock file is a crate of this workspace.
if grep -q '^source = ' Cargo.lock; then
    echo "Cargo.lock lists a package from outside the workspace:" >&2
    grep -B2 '^source = ' Cargo.lock >&2
    exit 1
fi

cargo fmt --check
cargo clippy --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --quiet

if [ "${1:-}" != "--quick" ]; then
    cargo test
fi
