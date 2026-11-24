# Working on onyxdb

onyxdb is a SQL database I'm building to understand how databases work inside.
Here's how I want you to work on it.

## Key points

- **Design first, with me.** Don't write code before we've agreed on the design.
  Every real choice is mine: a format, a trade-off, a promise to the layer
  above. Lay out the options and what each one costs, I decide, and the decision
  goes into [ARCHITECTURE.md](ARCHITECTURE.md) before the code: what we chose,
  why, and what it costs.
- **Tests first.** Write the model test, the crash test and the fuzz test before
  the code they check.
- **Walk me through every change:** the design it follows, the tests that prove
  it, and the 2 or 3 places where the important decisions live. I want to
  understand it, not just see it pass.
- **std only.** No dependencies except the crates in this workspace.
- **Commit only when I ask.** Commits follow Conventional Commits.

## Layers

- **Dependencies point down.** A crate never uses one above it. When a lower
  crate needs something from above, it defines a trait and the crate above
  implements it.
- **Change below, fix above, one commit.** When you change a lower crate, fix
  every crate above it in the same commit. `cargo test` at the workspace root
  checks them all.
- **Final interfaces in the first part.** A crate that's built over several
  milestones gets its whole public API the first time. A later part changes
  what's inside the functions, not their signatures.
- **All file I/O goes through `vfs`.** No crate above `vfs` touches `std::fs`, so
  I can crash-test every crate that writes to disk at every write, and give it a
  failing `fsync`.
- **One error convention.** Each crate has its own `Error` enum and `Result`. A
  crate converts the errors of the crate below with `From` into a variant of its
  own, and never returns a lower crate's `Result` from its own API. It adds what
  it was doing only when the lower error doesn't already say it.
- **No `unsafe`.**
- When you add a crate, or its place among the others changes, update
  `README.md` and `ARCHITECTURE.md` in the same commit.

## Checks

I check every change two ways:

- **Against a model.** Random operations run on the code and on a plain
  in-memory model (a `Vec`, a `HashMap`, a `BTreeMap`), and the results must
  agree. The random generator is our own small seeded xorshift (`vfs::Rng`). A
  storage engine must pass the `kv` suite.
- **Against a real system,** wherever one does the same job: `sqlite3` for query
  results and key order, Hermitage's tables for isolation. When our design
  differs on purpose, compare what both should agree on.

And also:

- **Crash and corruption cases** for anything that writes to disk: crash at
  every write through `vfs::crash_test`, and fail every `fsync` once.
- **Fuzz everything that decodes bytes:** pages, log records, batches, wire
  messages and SQL text. Feed it random and mutated input from the same seeded
  generator. Decoders return errors; they never panic.
- **Benchmarks with a baseline.** A crate that cares about speed has a
  `benches/` binary on std only that prints its numbers, and a saved baseline. If
  a change makes a number worse, say so in its commit.

A change is done when these pass:

```text
cargo test
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

## Crate READMEs

Every crate has a `README.md` that describes the code as it is now: what the
crate is, a short example, its format, its decisions and its known gaps. Change
it in the same commit as the code. `lib.rs` starts with
`#![doc = include_str!("../README.md")]`, so the README is the crate's front
page in `cargo doc` and its example runs as a doctest. A crate's state goes only
in the crate table in the top `README.md`.

Each decision is one entry:

```text
- **A CRC-32C in the last 4 bytes of every page.**
  Why: it catches flipped bits and torn writes on every read.
  Cost: 4 bytes per page, and it can't catch a lost write.
```

Keep READMEs short: tables, small diagrams, no walls of text. Each one links
back to its topic in
[engineering-notes](https://github.com/PPRAMANIK62/engineering-notes/blob/main/database-references/README.md).

## Docs and comments

Document the code the way the Rust standard library is documented. Someone who
reads only `cargo doc`, or only the code, should understand what every item is,
how to use it, what can go wrong, and why it's built the way it is.

| Comment | Style |
|---|---|
| `///` item docs | Rust std: what the item does, then `# Errors`, `# Panics`, `# Examples`. Literal and plain: no "you", no contractions. |
| `//!` module header | A sentence on what the module does, a `# Key points` list (3 to 6 bullets, the term in bold where it's defined), a small worked example in a `text` block when it helps (the layout for bytes, the flow for calls), and `# Caveats` for its weak spots. |
| `//` inside a function | One plain "why" sentence, where a line encodes a format rule, guards a corrupt-file case, keeps a concurrency rule, or does something the code can't explain itself. Never narrate what the next line plainly does. |

- **The code explains the design it has:** what we chose, why, and what it
  costs, right where the choice is made. It doesn't argue against alternatives
  or describe other systems.
- **Each crate stands alone.** Comments never mention milestones, books, other
  crates' plans, or what a later part will add.
- **Sections,** each under its own heading: `# Errors` (every variant, and
  when), `# Panics`, `# Deadlocks` (for anything that takes a lock), and
  `# Examples`.
- **`# Examples`** is a doctest. Every public type, function and method has one,
  except std trait impls and trivial getters. A trait that outside code
  implements also shows a small implementation. Examples use `unwrap()`.
- **Fields and variants** each get a `///`. Fields of an on-disk structure start
  with their offset (`/// 16: bytes per page`). An error variant says when it
  happens.
- **Private items** get a `///` that says what they do and what the caller must
  guarantee, unless the name and signature already say everything.
- **Error messages** name the rule that broke, with the numbers involved.
- **Plain words.** Full sentences, common words, no metaphors, no idioms, no
  phrasal verbs, no em dashes. Every "it" points at something in the same
  paragraph, and every "because" is a whole step.

## Commits

[Conventional Commits](https://www.conventionalcommits.org/): `type(scope):
summary` on one short lowercase line, no body, no file names. The type is
`feat`, `fix`, `docs`, `test`, `refactor`, `perf` or `chore`, and the scope is
the crate or area: `feat(wal): drop old segments from the log`.
