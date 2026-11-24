# Coding standards

The rules a review checks in onyxdb. `scripts/check.sh` already enforces the
mechanical ones, so a review can skip them: std only, formatting, every clippy
warning, `unsafe` (forbidden in `Cargo.toml`), `std::fs` outside `vfs`
(`clippy.toml`), a doc comment on every public item, and an `# Errors` or
`# Panics` section where a function needs one.

## Design

onyxdb follows John Ousterhout's *A Philosophy of Software Design*: the goal
is less complexity, and the main tool is deep modules that hide it.

- **Deep modules.** A module's interface is much simpler than what it does.
  `Kv` is three methods over a whole storage engine; `crash_test` is one
  function over every power cut and seed. A new public item has to earn its
  place in the interface.
- **Information hiding.** Each design decision lives in one module. When two
  modules must both know a format or a rule, one of them owns it and the other
  asks through the interface.
- **Pull complexity down.** The module does the hard part once, so its callers
  do not: `PageFile::write` seals the checksum, `Batch` checks the limits for
  every engine, `Wal::open` repairs the tail after a crash.
- **Define errors out of existence.** Prefer a meaning that has no error case
  (an empty range returns nothing; the root always exists) over an error every
  caller must handle. A rule the caller must remember, such as "pass the same
  number twice", is a sign that the interface should change.
- **General-purpose interfaces, special-purpose code beside them.** Traits such
  as `Vfs`, `PageStore`, `LogHook` and `Kv` know nothing about their callers.
- **Different layer, different abstraction.** A method that only passes its
  arguments to the layer below adds an interface and no depth.
- **Design it twice.** A design session lays out at least two real options
  before a decision becomes an ADR.

The red flags a review looks for, from the book: a shallow module,
information leakage, temporal decomposition, overexposure, a pass-through
method, repetition, special-general mixture, conjoined methods, a comment that
repeats the code, implementation details in an interface comment, a vague
name, a name that is hard to pick, and code that is not obvious.

## Errors

- Each crate has its own `Error` enum and `Result`.
- A crate converts the errors of the crate below with `From` into a variant of
  its own. Its public API returns its own `Result`.
- A variant adds what the crate was doing only when the lower error does not
  already say it.
- An error message names the rule that broke, with the numbers involved: "page
  3 is damaged: its checksum is 1a2b3c4d, but its bytes give 5e6f7a8b".

## Tests

Each slice of work gets the tests that apply to it, written before its code:

- **Against a model.** Random operations run on the code and on a plain
  in-memory model (a `Vec`, a `HashMap`, a `BTreeMap`), and the results agree.
  The random generator is `vfs::Rng`, seeded, so a failure repeats exactly, and
  every seed loop takes its count from `vfs::seeds`, so `ONYXDB_SEEDS=10 cargo
  test` runs 10 times as many seeds. A storage engine passes the `kv` suite.
  A seed that found a bug becomes its own named test, so later changes to the
  seed ranges keep checking it.
- **Against a real system,** wherever one does the same job: `sqlite3` for query
  results and key order, Hermitage's tables for isolation. Where our design
  differs on purpose, the test compares what both must agree on.
- **Crashes,** for anything that writes to disk: a crash at every write through
  `vfs::crash_test`, and a failed `fsync` at every sync.
- **Fuzzing,** for everything that decodes bytes: pages, log records, batches,
  wire messages and SQL text. Random and mutated input from `vfs::Rng`. A
  decoder returns an error for bad input, and never panics.
- **Benchmarks,** for a crate that cares about speed: a `benches/` binary on std
  only that prints its numbers, and a saved `benches/baseline.txt` whose first
  line names the date, CPU, disk and compiler. A commit that makes a number
  worse, on the same machine, says so.

## Crate READMEs

- Every crate has a `README.md` that describes the code as it is now: what the
  crate is, a short example, its format and its known gaps. It changes in the
  same commit as the code.
- `lib.rs` starts with `#![doc = include_str!("../README.md")]`, so the README is
  the crate's front page in `cargo doc`, and its example runs as a doctest.
- A crate's state (done, planned) goes only in the crate table of the top
  `README.md`. Decisions go only in `docs/adr/`.
- Short: tables and small diagrams. Each README links back to its topic in
  [engineering-notes](https://github.com/PPRAMANIK62/engineering-notes/blob/main/database-references/README.md).

## Docs and comments

Document the code the way the Rust standard library is documented. Someone who
reads only `cargo doc`, or only the code, understands what every item is, how
to use it, what can go wrong, and why it is built the way it is.

| Comment | Style |
|---|---|
| `///` item docs | What the item does, then `# Errors`, `# Panics`, `# Deadlocks` and `# Examples`. |
| `//!` module header | A sentence on what the module does, a `# Key points` list (3 to 6 bullets, the term in bold where it is defined), a small worked example in a `text` block when it helps (the layout for bytes, the flow for calls), and `# Caveats` for its weak spots. |
| `//` inside a function | One "why" sentence, where a line encodes a format rule, guards a corrupt-file case, keeps a concurrency rule, or does something the code cannot explain itself. |

- **The code explains the design it has:** what we chose, why, and what it
  costs, at the place where the choice is made.
- **Each crate stands alone.** Comments describe this crate as it is now: no
  milestones, books, other crates' plans or later parts.
- **`# Errors`** lists every variant the function returns, and when.
  **`# Deadlocks`** goes on anything that takes a lock.
- **`# Examples`** is a doctest on every public type, function and method, except
  std trait impls and trivial getters. A trait that outside code implements also
  shows a small implementation. Examples use `unwrap()`.
- **Fields and variants** each get a `///`. A field of an on-disk structure
  starts with its offset (`/// 16: bytes per page`). An error variant says when
  it happens.
- **Private items** get a `///` that says what they do and what the caller must
  guarantee, when the name and signature do not already say it.
- **Plain words.** Full sentences, common words and the literal meaning: no
  metaphors, idioms or phrasal verbs, no em dashes, no "you" and no
  contractions in doc comments. Every "it" points at something in the same
  paragraph, and every "because" is a whole step.
