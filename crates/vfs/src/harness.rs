//! `crash_test`: crashes a workload at every point and checks what is left.
//!
//! The workload cuts the power after its first change, then after its second,
//! and so on, and each cut is crashed with many seeds. As it runs, the
//! workload records what it was told is safe (its acknowledgements) in a
//! value of type `A`. The check gets that value with the crashed file system,
//! so it can tell data that was promised and lost from data that was never
//! promised.
//!
//! ```text
//! for n in 0..=changes, for seed in 0..seeds:
//!   fresh Mem -> power_off_after(n) -> workload -> crash(seed) -> check
//! ```
//!
//! A workload of 20 changes with 10 seeds is checked after 210 crashes.
//! Each one starts from an empty file system, so they do not affect each
//! other.

use std::fmt;

use crate::{Error, Mem, Result};

/// One crash after which the check failed. `changes` and `seed` are enough
/// to repeat it exactly: run the workload on a fresh `Mem` with
/// `power_off_after(changes)`, then `crash(seed)`.
#[derive(Debug)]
pub struct Failure {
    /// How many changes the workload made before the power went off.
    pub changes: u64,
    /// The seed the crash was chosen with.
    pub seed: u64,
    /// What the check said was wrong.
    pub message: String,
}

/// The result of a [`crash_test`]. The workload passed if `failures` is
/// empty. Printing it with `{}` shows the counts and the first failure.
#[derive(Debug)]
pub struct Report {
    /// How many crashes were checked: the number of change counts times
    /// the number of seeds.
    pub runs: u64,
    /// The crashes after which the check failed.
    pub failures: Vec<Failure>,
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} crashes, {} failed", self.runs, self.failures.len())?;
        if let Some(first) = self.failures.first() {
            write!(
                f,
                "\nfirst: power off after {} changes, seed {}: {}",
                first.changes, first.seed, first.message
            )?;
        }
        Ok(())
    }
}

/// Crash-tests `workload`. First it runs once with no crash, to count its
/// changes, and `check` must pass. Then, for each `n` from 0 to that count
/// and each seed below `seeds`: run on a fresh `Mem` that loses power after
/// `n` changes, crash it with the seed, and run `check`.
///
/// `workload` gets the file system and the acknowledgements. It records
/// what it was told is durable in them, and it should stop at the first
/// error, as a real program would when the machine stops. `check` gets the
/// crashed file system and those acknowledgements, and returns `Err` with a
/// message if anything acknowledged is missing or wrong.
///
/// The workload must do the same changes in the same order on every run,
/// or the change counts mean different things from run to run.
///
/// `workload` and `check` are closures. `impl Fn(&Mem, &mut A) -> Result<()>`
/// accepts any closure or function with that signature. They are `Fn`, not
/// `FnMut`, because they run many times and must not carry state from one
/// run to the next. Everything a run learns goes in `A`, which starts fresh
/// from `A::default()` every time.
///
/// # Panics
///
/// Panics if the workload fails, or the check fails, on the run with no
/// crash. Then the workload or the check is wrong, not the crash handling.
///
/// # Examples
///
/// ```
/// use std::path::Path;
/// use vfs::{Mem, Open, Vfs, crash_test};
///
/// // Writes a file, and acknowledges it once it's synced.
/// let workload = |mem: &Mem, synced: &mut bool| {
///     let file = mem.open(Path::new("a"), Open::CreateNew)?;
///     file.write_at(b"data", 0)?;
///     file.sync()?;
///     mem.sync_dir(Path::new(""))?;
///     *synced = true;
///     Ok(())
/// };
/// // An acknowledged file must hold its data after any crash.
/// let check = |mem: &Mem, synced: &bool| {
///     if !*synced {
///         return Ok(());
///     }
///     let file = mem.open(Path::new("a"), Open::Existing).map_err(|e| e.to_string())?;
///     let mut buf = [0; 4];
///     file.read_at(&mut buf, 0).map_err(|e| e.to_string())?;
///     if &buf == b"data" { Ok(()) } else { Err(format!("read {buf:?}")) }
/// };
/// let report = crash_test(5, workload, check);
/// assert!(report.failures.is_empty(), "{report}");
/// // 4 changes, so 5 power cuts, each with 5 seeds.
/// assert_eq!(report.runs, 25);
/// ```
pub fn crash_test<A: Default>(
    seeds: u64,
    workload: impl Fn(&Mem, &mut A) -> Result<()>,
    check: impl Fn(&Mem, &A) -> std::result::Result<(), String>,
) -> Report {
    let full = Mem::new();
    let mut acks = A::default();
    if let Err(e) = workload(&full, &mut acks) {
        panic!("the workload fails with no crash at all: {e}");
    }
    if let Err(message) = check(&full, &acks) {
        panic!("the check fails with no crash at all: {message}");
    }
    let total = full.changes();

    let mut report = Report {
        runs: 0,
        failures: Vec::new(),
    };
    for changes in 0..=total {
        for seed in 0..seeds {
            let mem = Mem::new();
            mem.power_off_after(changes);
            let mut acks = A::default();
            let result = match workload(&mem, &mut acks) {
                // The workload stopped at the power cut, as it should.
                Ok(()) | Err(Error::PowerOff) => {
                    mem.crash(seed);
                    check(&mem, &acks)
                }
                Err(e) => Err(format!(
                    "the workload failed with something other than the power cut: {e}"
                )),
            };
            report.runs += 1;
            if let Err(message) = result {
                report.failures.push(Failure {
                    changes,
                    seed,
                    message,
                });
            }
        }
    }
    report
}
