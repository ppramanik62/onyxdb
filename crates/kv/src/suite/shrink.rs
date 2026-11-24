//! `shrink` and `check`: cutting a failing test down to the steps that
//! matter.
//!
//! A random test that fails usually has hundreds of steps, and only a few
//! of them cause the failure. `shrink` removes steps and ops and keeps
//! every removal after which the test still fails:
//!
//! 1. It removes chunks of steps: the first half, then the second half,
//!    then quarters, and so on down to single steps.
//! 2. It makes each remaining step smaller: one op out of a batch, one
//!    batch out of the writes of a scan, or `sync` off.
//! 3. It repeats both until a whole round removes nothing.
//!
//! Every removal makes the test strictly smaller, so shrinking always
//! ends. The result is not always the smallest failing test, but each step
//! and each op in it is needed: without any one of them, the test passes.
//!
//! This is a simple form of delta debugging. It needs no knowledge of the
//! failure, only a function that says whether a list of steps still fails.

use std::fmt;

use super::{Engine, Step, generate, run};

/// Removes steps and ops from `steps` while `fails` still returns `true`,
/// and returns the smaller list. `fails` must return `true` for `steps`
/// itself.
///
/// # Examples
///
/// A test that "fails" whenever it has a get of `b"x"`. Shrinking keeps
/// only that step.
///
/// ```
/// use kv::suite::{Step, generate, shrink};
///
/// let mut steps = generate(3, 50);
/// steps.insert(20, Step::Get(b"x".to_vec()));
/// let fails = |steps: &[Step]| steps.contains(&Step::Get(b"x".to_vec()));
/// assert_eq!(shrink(steps, fails), [Step::Get(b"x".to_vec())]);
/// ```
pub fn shrink(mut steps: Vec<Step>, fails: impl Fn(&[Step]) -> bool) -> Vec<Step> {
    loop {
        let before = size(&steps);

        let mut chunk = steps.len().div_ceil(2);
        while chunk > 0 {
            let mut i = 0;
            while i < steps.len() {
                let mut candidate = steps.clone();
                candidate.drain(i..(i + chunk).min(steps.len()));
                if fails(&candidate) {
                    steps = candidate;
                } else {
                    i += chunk;
                }
            }
            chunk /= 2;
        }

        for i in 0..steps.len() {
            // After each removal that keeps the failure, try every smaller
            // form of this step again, starting from the new step.
            while let Some(smaller) = smaller(&steps[i]).into_iter().find(|smaller| {
                let mut candidate = steps.clone();
                candidate[i] = smaller.clone();
                fails(&candidate)
            }) {
                steps[i] = smaller;
            }
        }

        if size(&steps) == before {
            return steps;
        }
    }
}

/// The size that shrinking makes smaller: the steps, the ops in their
/// batches, the batches written during scans, and the `sync` flags.
fn size(steps: &[Step]) -> usize {
    let size = |step: &Step| match step {
        Step::Apply(batch, sync) => 1 + batch.len() + *sync as usize,
        Step::ScanWhile { writes, .. } => {
            1 + writes.len() + writes.iter().map(|b| b.len()).sum::<usize>()
        }
        _ => 1,
    };
    steps.iter().map(size).sum()
}

/// Every form of `step` with one thing removed: one op, one batch of
/// writes, or `sync`.
fn smaller(step: &Step) -> Vec<Step> {
    let mut out = Vec::new();
    match step {
        Step::Apply(batch, sync) => {
            if *sync {
                out.push(Step::Apply(batch.clone(), false));
            }
            for i in 0..batch.len() {
                let mut batch = batch.clone();
                batch.ops.remove(i);
                out.push(Step::Apply(batch, *sync));
            }
        }
        Step::ScanWhile { from, to, writes } => {
            let with = |writes: Vec<_>| Step::ScanWhile {
                from: from.clone(),
                to: to.clone(),
                writes,
            };
            for i in 0..writes.len() {
                let mut fewer = writes.clone();
                fewer.remove(i);
                out.push(with(fewer));
                for j in 0..writes[i].len() {
                    let mut fewer = writes.clone();
                    fewer[i].ops.remove(j);
                    out.push(with(fewer));
                }
            }
        }
        _ => {}
    }
    out
}

/// A failed test, shrunk: the seed that generated it, the steps that are
/// left, and what went wrong when they ran.
///
/// Printed with `{}`, it shows the message and then the steps, one per
/// line, numbered from 0.
#[derive(Debug)]
pub struct Failure {
    /// The seed that [`generate`] made the failing test from.
    pub seed: u64,
    /// The steps left after shrinking. They still fail.
    pub steps: Vec<Step>,
    /// What went wrong when the shrunk steps ran.
    pub message: String,
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let plural = if self.steps.len() == 1 { "" } else { "s" };
        let (seed, len, message) = (self.seed, self.steps.len(), &self.message);
        writeln!(f, "seed {seed}, shrunk to {len} step{plural}: {message}")?;
        for (i, step) in self.steps.iter().enumerate() {
            writeln!(f, "  {i}: {step}")?;
        }
        Ok(())
    }
}

/// Runs `len` generated steps for each seed in [`vfs::seeds(seeds)`](vfs::seeds). At the
/// first seed that fails, shrinks its steps and returns them.
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
/// use kv::suite::check;
///
/// check(&ModelEngine, 5, 100).unwrap();
/// ```
pub fn check<E: Engine>(engine: &E, seeds: u64, len: usize) -> Result<(), Failure> {
    for seed in vfs::seeds(seeds) {
        let steps = generate(seed, len);
        if run(engine, &steps).is_ok() {
            continue;
        }
        let steps = shrink(steps, |steps| run(engine, steps).is_err());
        // The shrunk steps still fail, so this is always `Err`.
        let message = run(engine, &steps).err().unwrap_or_default();
        return Err(Failure {
            seed,
            steps,
            message,
        });
    }
    Ok(())
}
