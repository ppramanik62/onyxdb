# Working on onyxdb

onyxdb is a SQL database I'm building to understand how databases work inside.
I make the decisions; you build them and explain them to me.

## Before you start

Read [CONTEXT.md](CONTEXT.md) for the terms, and the ADRs in
[docs/adr](docs/adr) that touch the crates you are about to change.

## How a change goes

1. **Design with me.** Every real choice is mine: a format, a trade-off, a
   promise to the layer above. Lay out the options and what each one costs. I
   decide, and the decision becomes an ADR in `docs/adr/` before the code. New
   terms go into `CONTEXT.md`.
2. **Build test-first, one slice at a time:** one failing test, then the code
   that makes it pass, then the next test. The kinds of test each slice needs
   are in [CODING_STANDARDS.md](CODING_STANDARDS.md#tests).
3. **Run one test file while you work:** `cargo test -p <crate> --test <file>`.
   The whole suite takes about a minute, most of it in the crash tests.
4. **Done when `scripts/check.sh` passes.** It checks that the workspace uses
   the standard library only, then runs `cargo fmt --check`, clippy with
   `-D warnings`, `cargo doc` with `-D warnings`, and `cargo test`. CI runs the
   same script. The pre-commit hook runs it without the tests; turn the hook on
   once per clone with `git config core.hooksPath .githooks`.
5. **Walk me through the change:** the design it follows, the tests that prove
   it, and the 2 or 3 places where the important decisions live. I want to
   understand it, not only see it pass.
6. **Commit when I ask.** [Conventional Commits](https://www.conventionalcommits.org/):
   `type(scope): summary` on one short lowercase line, with no body and no file
   names. The type is `feat`, `fix`, `docs`, `test`, `refactor`, `perf` or
   `chore`, and the scope is the crate or area: `feat(wal): drop old segments
   from the log`.

## Layers

- **Dependencies point down.** When a lower crate needs something from above,
  it defines a trait and the crate above implements it.
- **Change below, fix above, one commit.** A change to a lower crate fixes every
  crate above it in the same commit.
- **A crate gets its whole public API in its first part.** Later parts change
  what is inside the functions, and keep their signatures.
- **File I/O goes through `vfs`,** so every crate that writes to disk can be
  crashed at every write. `clippy.toml` holds the list of `std::fs` calls that
  clippy rejects.
- **Adding or moving a crate** updates the crate table and the layer diagram in
  `README.md` in the same commit.

## Code

[CODING_STANDARDS.md](CODING_STANDARDS.md) holds the rules for errors, tests,
crate READMEs, docs and comments. Read it before you write any of these. The
review checks every change against it.
