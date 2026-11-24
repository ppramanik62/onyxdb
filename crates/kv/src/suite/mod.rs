//! Tests that any engine can run, to show that it keeps every promise of
//! [`Kv`].
//!
//! A test is a list of [`Step`]s. [`run`] performs the steps on the engine
//! and on a plain `BTreeMap`, which is the expected answer, and fails at
//! the first answer that differs. [`generate`] makes random steps from a
//! seed, aimed at the edges where engines go wrong. [`shrink`] cuts a
//! failing list down to the few steps that still fail, and [`check`] does
//! all three for many seeds. [`crashes`] performs the writes of a list on
//! a file system that loses power after every change, and checks what the
//! engine holds afterwards. [`threads`] runs writers in many threads.
//!
//! | Term | Meaning |
//! |---|---|
//! | step | One operation of a test: a read, a scan, a batch, or opening the engine again. |
//! | expected | The `BTreeMap` that receives the same writes as the engine. |
//! | prefix | The first `k` batches of a test, for some `k`. After a crash, the engine must hold exactly one prefix. |
//! | shrink | Remove steps and ops from a failing test while it still fails. |
//!
//! ```text
//!  generate(seed) ──▶ steps ──▶ run(engine, steps) ──▶ Ok
//!                                      │ Err
//!                                      ▼
//!                       shrink(steps, still fails?) ──▶ a few steps
//! ```
//!
//! Every test runs the engine on a fresh `vfs::Mem`, so nothing touches the
//! real disk and a failure repeats exactly.

use std::{collections::BTreeMap, fmt, ops::Bound, sync::Arc};

use vfs::Vfs;

use crate::{Batch, Kv, Op, Result, empty_range};

/// How the suite makes the engine under test: create it on an empty file
/// system, and open it again on the same one.
///
/// The engine chooses its own paths and settings. A test usually picks
/// small settings, such as small pages or segments, so that a short test
/// reaches the edges of the engine.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use kv::{Kv, Model, Result, suite::Engine};
/// use vfs::{Mem, Vfs};
///
/// struct ModelEngine;
///
/// impl Engine for ModelEngine {
///     type Kv = Model;
///     fn create(&self, vfs: Arc<dyn Vfs>) -> Result<Model> {
///         Model::create(vfs, "kv")
///     }
///     fn open(&self, vfs: Arc<dyn Vfs>) -> Result<Model> {
///         Model::open(vfs, "kv")
///     }
/// }
///
/// // The suite calls these with a fresh `Mem` for every test.
/// let mem: Arc<dyn Vfs> = Arc::new(Mem::new());
/// let kv = ModelEngine.create(mem.clone()).unwrap();
/// drop(kv);
/// let kv = ModelEngine.open(mem).unwrap();
/// assert_eq!(kv.get(b"a").unwrap(), None);
/// ```
pub trait Engine {
    /// The engine's type.
    type Kv: Kv;

    /// Creates an empty engine on `vfs`, which has no files yet.
    ///
    /// # Errors
    ///
    /// Any error the engine had while it created its files.
    fn create(&self, vfs: Arc<dyn Vfs>) -> Result<Self::Kv>;

    /// Opens the engine that `create` made on `vfs`, after a clean drop or
    /// after a crash.
    ///
    /// # Errors
    ///
    /// Any error the engine had while it opened and recovered.
    fn open(&self, vfs: Arc<dyn Vfs>) -> Result<Self::Kv>;
}

/// One operation of a test.
///
/// Printed with `{}`, a step is one line, such as
/// `apply sync [put "a" = "1", delete "b"]`. Keys and values are printed
/// with Rust's byte escapes, and a value longer than 16 bytes is printed as
/// its length.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Read one key.
    Get(Vec<u8>),
    /// Scan a range, from the first bound to the second.
    Scan(Bound<Vec<u8>>, Bound<Vec<u8>>),
    /// Scan a range, and apply the next of `writes` after each item the
    /// scan returns. The writes left when the scan ends are applied after
    /// it.
    ScanWhile {
        /// The scan's start.
        from: Bound<Vec<u8>>,
        /// The scan's end.
        to: Bound<Vec<u8>>,
        /// The batches to apply during the scan, without `sync`.
        writes: Vec<Batch>,
    },
    /// Apply a batch, with `sync` if the flag is set.
    Apply(Batch, bool),
    /// Drop the engine and open it again, with no crash.
    Reopen,
}

impl fmt::Display for Step {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Step::Get(key) => write!(f, "get {}", show(key)),
            Step::Scan(from, to) => write!(f, "scan {}", show_range(from, to)),
            Step::ScanWhile { from, to, writes } => {
                let writes: Vec<_> = writes.iter().map(batch).collect();
                write!(
                    f,
                    "scan {} writing {}",
                    show_range(from, to),
                    writes.join(", ")
                )
            }
            Step::Apply(b, true) => write!(f, "apply sync {}", batch(b)),
            Step::Apply(b, false) => write!(f, "apply {}", batch(b)),
            Step::Reopen => write!(f, "reopen"),
        }
    }
}

/// Prints bytes in quotes, with Rust's escapes for bytes that are not
/// printable ASCII. Bytes longer than 16 are printed as their length.
pub(crate) fn show(bytes: &[u8]) -> String {
    match bytes.len() {
        0..=16 => format!("\"{}\"", bytes.escape_ascii()),
        len => format!("<{len} bytes>"),
    }
}

/// Prints a range: `[` or `]` for an included bound, `(` or `)` for an
/// excluded one, and `..` for no bound.
fn show_range(from: &Bound<Vec<u8>>, to: &Bound<Vec<u8>>) -> String {
    let from = match from {
        Bound::Included(k) => format!("[{}", show(k)),
        Bound::Excluded(k) => format!("({}", show(k)),
        Bound::Unbounded => "(..".to_string(),
    };
    let to = match to {
        Bound::Included(k) => format!("{}]", show(k)),
        Bound::Excluded(k) => format!("{})", show(k)),
        Bound::Unbounded => "..)".to_string(),
    };
    format!("{from}, {to}")
}

/// Prints a batch as a list of its ops.
fn batch(batch: &Batch) -> String {
    let ops: Vec<_> = batch
        .ops()
        .iter()
        .map(|op| match op {
            Op::Put(k, v) => format!("put {} = {}", show(k), show(v)),
            Op::Delete(k) => format!("delete {}", show(k)),
        })
        .collect();
    format!("[{}]", ops.join(", "))
}

/// The expected contents of an engine: every key and its value.
pub(crate) type Map = BTreeMap<Vec<u8>, Vec<u8>>;

/// Applies a batch to the expected contents, op by op.
pub(crate) fn apply_to(map: &mut Map, batch: &Batch) {
    for op in batch.ops() {
        match op {
            Op::Put(key, value) => map.insert(key.clone(), value.clone()),
            Op::Delete(key) => map.remove(key),
        };
    }
}

/// Borrows an owned bound as the `Bound<&[u8]>` that `Kv::scan` takes.
pub(crate) fn borrow(bound: &Bound<Vec<u8>>) -> Bound<&[u8]> {
    bound.as_ref().map(Vec::as_slice)
}

/// What a scan of the expected contents returns, with the same rule for
/// empty ranges as an engine.
pub(crate) fn expected_scan(
    map: &Map,
    from: &Bound<Vec<u8>>,
    to: &Bound<Vec<u8>>,
) -> Vec<(Vec<u8>, Vec<u8>)> {
    let (from, to) = (borrow(from), borrow(to));
    if empty_range(from, to) {
        return Vec::new();
    }
    map.range::<[u8], _>((from, to))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// Scans the whole engine and checks that it holds exactly `expected`.
/// The error names the first key where the two differ.
pub(crate) fn same_contents(kv: &impl Kv, expected: &Map) -> std::result::Result<(), String> {
    let mut got = Vec::new();
    for item in kv
        .scan(Bound::Unbounded, Bound::Unbounded)
        .map_err(|e| e.to_string())?
    {
        got.push(item.map_err(|e| e.to_string())?);
    }
    let want: Vec<_> = expected
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    first_difference(&got, &want)
}

/// Compares the items a scan returned with the items expected, and names
/// the first difference.
pub(crate) fn first_difference(
    got: &[(Vec<u8>, Vec<u8>)],
    want: &[(Vec<u8>, Vec<u8>)],
) -> std::result::Result<(), String> {
    let item = |i: Option<&(Vec<u8>, Vec<u8>)>| match i {
        Some((k, v)) => format!("{} = {}", show(k), show(v)),
        None => "nothing".to_string(),
    };
    match (0..got.len().max(want.len())).find(|&i| got.get(i) != want.get(i)) {
        None => Ok(()),
        Some(i) => Err(format!(
            "item {i} is {}, expected {} ({} items, expected {})",
            item(got.get(i)),
            item(want.get(i)),
            got.len(),
            want.len()
        )),
    }
}

mod generate;
mod run;

pub use generate::generate;
pub use run::{run, threads};

mod shrink;

pub use shrink::{Failure, check, shrink};

mod crash;

pub use crash::{check_crashes, crashes};
