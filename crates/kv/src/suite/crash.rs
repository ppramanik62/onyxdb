//! `crashes` and `check_crashes`: cutting the power at every write, and
//! failing every sync once.
//!
//! [`crashes`] takes the writes of a test, its batches and reopens, and
//! runs them through `vfs::crash_test`: the power goes off after the
//! first change to the file system, then after the second, and so on, and
//! each cut is crashed with several seeds. After each crash, the engine
//! opens again, and the check holds it to the crash promise:
//!
//! ```text
//!  batches applied:   b0  b1  b2  b3  b4  b5
//!                             ▲               ▲
//!                    last synced (b2)      last attempted (b5)
//!
//!  the engine must hold exactly b0..b2, b0..b3, b0..b4 or b0..b5
//! ```
//!
//! Checking only that the synced batches survived would pass an engine
//! that loses `b3` and keeps `b4`. So the check builds the expected
//! contents after each prefix, from the last synced batch to the last one
//! attempted, and the engine must equal one of them. A batch in part
//! equals none of them.
//!
//! The same writes then run once for each sync, with that sync failing.
//! After a failed sync the test opens the engine again, as a program
//! would, and continues. The failed sync may have lost the batches since
//! the sync before it, so the test reads which prefix the engine holds
//! and continues from there. Every crash after that is checked in the same
//! way.
//!
//! After each check, the engine applies one more batch with `sync` and
//! opens again, and that batch must be there. A recovered engine that
//! reads correctly but cannot write is still broken.

use std::{io, path::PathBuf, sync::Arc};

use vfs::{Fault, Mem, Vfs, crash_test};

use crate::{Batch, Error, Kv};

use super::{Engine, Failure, Map, Step, apply_to, first_difference, generate, shrink};

/// How many seeds `check_crashes` crashes each power cut with.
const CRASH_SEEDS: u64 = 4;

/// What one run of the workload promised. The check after a crash holds
/// the engine to it.
#[derive(Default)]
struct Promised {
    /// `create` returned. A crash before that may leave no engine at all.
    created: bool,
    /// The numbers of the batches the engine holds, in order, ending with
    /// the batch being applied when the power went off. Batch numbers
    /// count the writes of the test from 0.
    history: Vec<usize>,
    /// How many batches at the start of `history` must survive a crash.
    synced: usize,
}

/// The writes of a test: its batches, the writes of its scans as batches
/// without `sync`, and its reopens. Reads change nothing on disk, so they
/// are left out.
fn writes(steps: &[Step]) -> Vec<Step> {
    let mut out = Vec::new();
    for step in steps {
        match step {
            Step::Apply(..) | Step::Reopen => out.push(step.clone()),
            Step::ScanWhile { writes, .. } => {
                out.extend(writes.iter().map(|b| Step::Apply(b.clone(), false)));
            }
            Step::Get(_) | Step::Scan(..) => {}
        }
    }
    out
}

/// Turns an error into the file system error that `crash_test` expects
/// from a workload. A file system error stays as it is, so the power cut
/// is seen as one. Anything else becomes an `Io` error with the message.
fn to_vfs(e: Error) -> vfs::Error {
    match e {
        Error::Vfs(e) => e,
        other => message(other.to_string()),
    }
}

/// A file system error that carries a message from the suite.
fn message(text: String) -> vfs::Error {
    vfs::Error::Io {
        op: "kv suite",
        path: PathBuf::new(),
        source: io::Error::other(text),
    }
}

/// Performs `actions` (from [`writes`]) on a new engine on `mem`, and
/// records in `promised` what the engine was told is safe. If `fail` is
/// `Some(n)`, the `n`th sync fails, counting only applies with `sync` of
/// batches that are not empty.
fn workload<E: Engine>(
    engine: &E,
    actions: &[Step],
    batches: &[&Batch],
    mem: &Mem,
    promised: &mut Promised,
    fail: Option<usize>,
) -> vfs::Result<()> {
    let vfs: Arc<dyn Vfs> = Arc::new(mem.clone());
    let mut kv = engine.create(vfs.clone()).map_err(to_vfs)?;
    promised.created = true;
    let (mut syncs, mut number) = (0, 0);
    for action in actions {
        match action {
            Step::Reopen => {
                drop(kv);
                kv = engine.open(vfs.clone()).map_err(open_error)?;
                // What an engine holds after it opens survives a crash.
                promised.synced = promised.history.len();
            }
            Step::Apply(batch, sync) => {
                promised.history.push(number);
                number += 1;
                if *sync && !batch.is_empty() {
                    if fail == Some(syncs) {
                        mem.fail_next(Fault::SyncError);
                    }
                    syncs += 1;
                }
                match kv.apply(batch.clone(), *sync) {
                    Ok(()) if *sync => promised.synced = promised.history.len(),
                    Ok(()) => {}
                    Err(Error::Vfs(vfs::Error::Injected(_))) => {
                        // The failed sync: open again, without a retry
                        // of the batch, and find out what is left.
                        drop(kv);
                        kv = engine.open(vfs.clone()).map_err(open_error)?;
                        let left = holds_prefix(&kv, batches, &promised.history, promised.synced)
                            .map_err(message)?;
                        promised.history.truncate(left);
                        promised.synced = left;
                    }
                    Err(e) => return Err(to_vfs(e)),
                }
            }
            _ => unreachable!("writes() returns only applies and reopens"),
        }
    }
    Ok(())
}

/// Turns an error from opening the engine into the file system error that
/// `crash_test` expects. An injected sync failure that reaches `open` was
/// meant for an earlier `apply` with `sync`, so that `apply` did not sync.
fn open_error(e: Error) -> vfs::Error {
    match e {
        Error::Vfs(vfs::Error::Injected(_)) => message(NOT_SYNCED.to_string()),
        other => to_vfs(other),
    }
}

/// What the suite reports when a sync failure set for `apply` reaches a
/// later call instead.
const NOT_SYNCED: &str =
    "a sync set to fail was not used by the apply with sync before it, so that apply did not sync";

/// Finds which prefix of `history` the engine holds. The prefix is at least
/// `synced` batches long. Returns its length.
///
/// # Errors
///
/// A message if the engine holds no such prefix, with the first difference
/// from the longest one.
fn holds_prefix(
    kv: &impl Kv,
    batches: &[&Batch],
    history: &[usize],
    synced: usize,
) -> Result<usize, String> {
    let mut got = Vec::new();
    for item in kv
        .scan(std::ops::Bound::Unbounded, std::ops::Bound::Unbounded)
        .map_err(|e| e.to_string())?
    {
        got.push(item.map_err(|e| e.to_string())?);
    }
    let mut expected = Map::new();
    for &b in &history[..synced] {
        apply_to(&mut expected, batches[b]);
    }
    let as_items = |map: &Map| {
        map.iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<Vec<_>>()
    };
    for len in synced..=history.len() {
        if len > synced {
            apply_to(&mut expected, batches[history[len - 1]]);
        }
        if got == as_items(&expected) {
            return Ok(len);
        }
    }
    let difference = first_difference(&got, &as_items(&expected))
        .err()
        .unwrap_or_default();
    let all = history.len();
    Err(format!(
        "the engine must hold the first {synced} to {all} batches, and it holds none of these; against all {all}: {difference}"
    ))
}

/// Opens the engine after a crash and checks it: it holds a prefix of the
/// batches that includes every synced one, it can apply and sync one more
/// batch, and that batch is there after it opens again.
fn check<E: Engine>(
    engine: &E,
    batches: &[&Batch],
    mem: &Mem,
    promised: &Promised,
) -> Result<(), String> {
    let vfs: Arc<dyn Vfs> = Arc::new(mem.clone());
    let kv = match engine.open(vfs.clone()) {
        Ok(kv) => kv,
        // A crash inside `create` may leave no engine to open.
        Err(_) if !promised.created => return Ok(()),
        Err(Error::Vfs(vfs::Error::Injected(_))) => return Err(NOT_SYNCED.to_string()),
        Err(e) => return Err(format!("open after the crash: {e}")),
    };
    holds_prefix(&kv, batches, &promised.history, promised.synced)?;

    // No key from `generate` has these bytes, so the batch is new.
    let (key, value) = (b"after the crash", b"ok");
    let mut batch = Batch::new();
    batch
        .put(key.to_vec(), value.to_vec())
        .map_err(|e| e.to_string())?;
    kv.apply(batch, true)
        .map_err(|e| format!("apply after the crash: {e}"))?;
    drop(kv);
    let kv = engine
        .open(vfs)
        .map_err(|e| format!("open after writing again: {e}"))?;
    match kv.get(key).map_err(|e| e.to_string())? {
        Some(v) if v == value => Ok(()),
        _ => Err("a batch synced after the crash is gone after one more open".to_string()),
    }
}

/// Runs the writes of `steps` through a crash at every change to the file
/// system, with `seeds` seeds for each, and checks the engine after every
/// crash. Then it does the same once for each sync, with that sync
/// failing. Returns how many crashes it checked.
///
/// # Errors
///
/// A message for the first failure: which run it was, the power cut and
/// the seed, and what the check found.
///
/// # Examples
///
/// ```
/// # use std::sync::Arc;
/// # use kv::{Model, Result, suite::Engine};
/// # use vfs::Vfs;
/// # struct ModelEngine;
/// # impl Engine for ModelEngine {
/// #     type Kv = Model;
/// #     fn create(&self, vfs: Arc<dyn Vfs>) -> Result<Model> { Model::create(vfs, "kv") }
/// #     fn open(&self, vfs: Arc<dyn Vfs>) -> Result<Model> { Model::open(vfs, "kv") }
/// # }
/// use kv::suite::{crashes, generate};
///
/// let checked = crashes(&ModelEngine, &generate(2, 10), 2).unwrap();
/// assert!(checked > 0);
/// ```
pub fn crashes<E: Engine>(engine: &E, steps: &[Step], seeds: u64) -> Result<u64, String> {
    let actions = writes(steps);
    let batches: Vec<&Batch> = actions
        .iter()
        .filter_map(|a| match a {
            Step::Apply(batch, _) => Some(batch),
            _ => None,
        })
        .collect();
    let syncs = actions
        .iter()
        .filter(|a| matches!(a, Step::Apply(b, true) if !b.is_empty()))
        .count();

    let mut checked = 0;
    for fail in std::iter::once(None).chain((0..syncs).map(Some)) {
        let run = match fail {
            None => "crash at every write".to_string(),
            Some(n) => format!("sync {n} fails, then a crash at every write"),
        };
        let workload =
            |mem: &Mem, p: &mut Promised| workload(engine, &actions, &batches, mem, p, fail);
        let check = |mem: &Mem, p: &Promised| check(engine, &batches, mem, p);
        // `crash_test` panics if the run with no crash fails, so that run
        // happens here first, and a failure is returned instead.
        let (mem, mut promised) = (Mem::new(), Promised::default());
        workload(&mem, &mut promised).map_err(|e| format!("{run}: with no crash: {e}"))?;
        check(&mem, &promised).map_err(|e| format!("{run}: with no crash: {e}"))?;

        let report = crash_test(seeds, workload, check);
        checked += report.runs;
        if !report.failures.is_empty() {
            return Err(format!("{run}: {report}"));
        }
    }
    Ok(checked)
}

/// Like [`check`](super::check), with crashes: for each seed from 0 to
/// `seeds - 1`, runs [`crashes`] on `len` generated steps, with 4 seeds for
/// each power cut. At the first seed that fails, shrinks its steps.
///
/// # Errors
///
/// The shrunk [`Failure`] of the first seed whose steps failed.
///
/// # Examples
///
/// ```
/// # use std::sync::Arc;
/// # use kv::{Model, Result, suite::Engine};
/// # use vfs::Vfs;
/// # struct ModelEngine;
/// # impl Engine for ModelEngine {
/// #     type Kv = Model;
/// #     fn create(&self, vfs: Arc<dyn Vfs>) -> Result<Model> { Model::create(vfs, "kv") }
/// #     fn open(&self, vfs: Arc<dyn Vfs>) -> Result<Model> { Model::open(vfs, "kv") }
/// # }
/// use kv::suite::check_crashes;
///
/// check_crashes(&ModelEngine, 2, 10).unwrap();
/// ```
pub fn check_crashes<E: Engine>(engine: &E, seeds: u64, len: usize) -> Result<(), Failure> {
    for seed in 0..seeds {
        let steps = generate(seed, len);
        if crashes(engine, &steps, CRASH_SEEDS).is_ok() {
            continue;
        }
        let steps = shrink(steps, |steps| crashes(engine, steps, CRASH_SEEDS).is_err());
        let message = crashes(engine, &steps, CRASH_SEEDS)
            .err()
            .unwrap_or_default();
        return Err(Failure {
            seed,
            steps,
            message,
        });
    }
    Ok(())
}
