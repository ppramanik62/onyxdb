//! `run` and `threads`: performing steps on an engine and checking every
//! answer.
//!
//! `run` performs each step on the engine and on the expected `BTreeMap`.
//! A get or a scan must return exactly what the map returns. After a reopen
//! and at the end, a scan of the whole engine must equal the whole map.
//!
//! A scan with writes during it cannot be checked against one map, because
//! the contract lets it see each key as it is when the scan reaches it.
//! For each key, the check keeps every value the key had while the scan
//! ran, and the scan passes if:
//!
//! - its keys are in strictly increasing order, all inside the range;
//! - each value it returns is one that its key had at some moment of the
//!   scan;
//! - every key that no write touched during the scan is returned, with its
//!   value.
//!
//! `threads` cannot compare with one map either, because the order in
//! which threads apply their batches changes from run to run. Each thread
//! writes only keys that start with its own byte and checks its own keys
//! against its own map. At the end, the whole engine must equal all the
//! maps together, and that answer does not depend on the order.

use std::{
    collections::{BTreeMap, BTreeSet},
    ops::{Bound, RangeBounds},
    sync::Arc,
};

use vfs::{Mem, Rng, Vfs};

use crate::{Batch, Kv, Op};

use super::{
    Engine, Map, Step, apply_to, borrow, expected_scan, first_difference, same_contents, show,
};

/// Performs `steps` on a new engine on a fresh `vfs::Mem`, and checks every
/// answer against a `BTreeMap` that receives the same writes.
///
/// # Errors
///
/// A message for the first step where the engine answered differently,
/// returned an error, or broke a rule of the contract. It names the step's
/// number and the step.
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
/// use kv::suite::{generate, run};
///
/// run(&ModelEngine, &generate(1, 200)).unwrap();
/// ```
pub fn run<E: Engine>(engine: &E, steps: &[Step]) -> Result<(), String> {
    let vfs: Arc<dyn Vfs> = Arc::new(Mem::new());
    let mut kv = engine
        .create(vfs.clone())
        .map_err(|e| format!("create: {e}"))?;
    let mut expected = Map::new();
    for (i, step) in steps.iter().enumerate() {
        let at = |message: String| format!("step {i} ({step}): {message}");
        match step {
            Step::Get(key) => {
                let got = kv.get(key).map_err(|e| at(e.to_string()))?;
                let want = expected.get(key);
                if got.as_ref() != want {
                    let show = |v: Option<&Vec<u8>>| v.map_or("nothing".to_string(), |v| show(v));
                    return Err(at(format!(
                        "got {}, expected {}",
                        show(got.as_ref()),
                        show(want)
                    )));
                }
            }
            Step::Scan(from, to) => {
                let mut got = Vec::new();
                for item in kv
                    .scan(borrow(from), borrow(to))
                    .map_err(|e| at(e.to_string()))?
                {
                    got.push(item.map_err(|e| at(e.to_string()))?);
                }
                first_difference(&got, &expected_scan(&expected, from, to)).map_err(at)?;
            }
            Step::ScanWhile { from, to, writes } => {
                scan_while(&kv, &mut expected, from, to, writes).map_err(at)?;
            }
            Step::Apply(batch, sync) => {
                kv.apply(batch.clone(), *sync)
                    .map_err(|e| at(e.to_string()))?;
                apply_to(&mut expected, batch);
            }
            Step::Reopen => {
                // The engine may hold locks on its files, so the old one
                // must be gone before the new one opens.
                drop(kv);
                kv = engine.open(vfs.clone()).map_err(|e| at(e.to_string()))?;
                same_contents(&kv, &expected).map_err(at)?;
            }
        }
    }
    same_contents(&kv, &expected).map_err(|e| format!("at the end: {e}"))
}

/// Scans from `from` to `to`, applies the next of `writes` after each item
/// the scan returns, and checks the scan by the rules in the module header.
/// The writes left when the scan ends are applied after it. `expected`
/// receives every write.
fn scan_while(
    kv: &impl Kv,
    expected: &mut Map,
    from: &Bound<Vec<u8>>,
    to: &Bound<Vec<u8>>,
    writes: &[Batch],
) -> Result<(), String> {
    let before = expected_scan(expected, from, to);
    // Every value each key had while the scan ran, starting with the
    // values it had before.
    let mut had: BTreeMap<Vec<u8>, Vec<Vec<u8>>> = before
        .iter()
        .map(|(k, v)| (k.clone(), vec![v.clone()]))
        .collect();
    let mut touched = BTreeSet::new();
    let mut writes = writes.iter();
    let mut got: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    for item in kv
        .scan(borrow(from), borrow(to))
        .map_err(|e| e.to_string())?
    {
        got.push(item.map_err(|e| e.to_string())?);
        let Some(batch) = writes.next() else { continue };
        kv.apply(batch.clone(), false).map_err(|e| e.to_string())?;
        apply_to(expected, batch);
        for op in batch.ops() {
            let (Op::Put(key, _) | Op::Delete(key)) = op;
            touched.insert(key.clone());
            if let Op::Put(key, value) = op {
                had.entry(key.clone()).or_default().push(value.clone());
            }
        }
    }
    for batch in writes {
        kv.apply(batch.clone(), false).map_err(|e| e.to_string())?;
        apply_to(expected, batch);
    }

    for (i, (key, value)) in got.iter().enumerate() {
        if i > 0 && got[i - 1].0 >= *key {
            return Err(format!(
                "the scan returned {} after {}",
                show(key),
                show(&got[i - 1].0)
            ));
        }
        // A pair of bounds is a `RangeBounds`, which has `contains`.
        if !(borrow(from), borrow(to)).contains(key.as_slice()) {
            return Err(format!(
                "the scan returned {}, which is outside the range",
                show(key)
            ));
        }
        if !had.get(key).is_some_and(|values| values.contains(value)) {
            return Err(format!(
                "the scan returned {} = {}, a value the key never had",
                show(key),
                show(value)
            ));
        }
    }
    for (key, value) in &before {
        let returned = got.iter().any(|(k, v)| k == key && v == value);
        if !touched.contains(key) && !returned {
            return Err(format!(
                "the scan skipped {} = {}, which no write touched",
                show(key),
                show(value)
            ));
        }
    }
    Ok(())
}

/// Runs `threads` writer threads on one engine, each performing `len`
/// random batches on keys that only it writes, and checks the result.
///
/// Thread `t` writes only keys whose first byte is `t`. After each batch it
/// reads one of its keys and checks it against its own map. Every tenth
/// batch, it scans the whole engine, which holds the keys of the other
/// threads too, and checks that the keys are in order. It also scans its
/// own keys and checks them against its own map. At the end, the whole
/// engine must equal all the maps together.
///
/// # Errors
///
/// A message for the first wrong answer, or the first error the engine
/// returned, with the thread's number.
///
/// # Panics
///
/// If `threads` is more than 256, because each thread needs its own first
/// byte.
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
/// use kv::suite::threads;
///
/// threads(&ModelEngine, 4, 100).unwrap();
/// ```
pub fn threads<E: Engine>(engine: &E, threads: usize, len: usize) -> Result<(), String> {
    assert!(
        threads <= 256,
        "{threads} threads, but only 256 first bytes"
    );
    let kv = engine
        .create(Arc::new(Mem::new()))
        .map_err(|e| format!("create: {e}"))?;
    // `std::thread::scope` lets the threads borrow `kv`, because every
    // thread ends before `scope` returns.
    let maps = std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads)
            .map(|t| {
                let kv = &kv;
                s.spawn(move || writer(kv, t as u8, len).map_err(|e| format!("thread {t}: {e}")))
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("a writer thread panicked"))
            .collect::<Result<Vec<_>, _>>()
    })?;
    let all: Map = maps.into_iter().flatten().collect();
    same_contents(&kv, &all).map_err(|e| format!("at the end: {e}"))
}

/// The work of one thread in [`threads`]: `len` random batches on keys that
/// start with `first`, each checked as [`threads`] describes. Returns the
/// thread's own map.
fn writer(kv: &impl Kv, first: u8, len: usize) -> Result<Map, String> {
    let mut rng = Rng::new(first as u64);
    let mut mine = Map::new();
    let key = |rng: &mut Rng| [vec![first], super::generate::short_key(rng)].concat();
    for i in 0..len {
        let mut batch = Batch::new();
        for _ in 0..1 + rng.below(4) {
            let k = key(&mut rng);
            let result = match rng.below(10) {
                0..=6 => batch.put(k, super::generate::value(&mut rng)),
                _ => batch.delete(k),
            };
            // A short key is at most 4 bytes here, far under the limit.
            result.unwrap();
        }
        kv.apply(batch.clone(), rng.below(10) == 0)
            .map_err(|e| e.to_string())?;
        apply_to(&mut mine, &batch);

        let k = key(&mut rng);
        let got = kv.get(&k).map_err(|e| e.to_string())?;
        if got.as_ref() != mine.get(&k) {
            return Err(format!("get {} is wrong after batch {i}", show(&k)));
        }
        if i % 10 == 9 {
            let mut last: Option<Vec<u8>> = None;
            for item in kv
                .scan(Bound::Unbounded, Bound::Unbounded)
                .map_err(|e| e.to_string())?
            {
                let (k, _) = item.map_err(|e| e.to_string())?;
                if last.as_ref().is_some_and(|last| *last >= k) {
                    return Err(format!("a full scan returned {} out of order", show(&k)));
                }
                last = Some(k);
            }
            // The keys of this thread: from [first] up to [first + 1],
            // or to the end for the last first byte.
            let to = match first.checked_add(1) {
                Some(next) => Bound::Excluded(vec![next]),
                None => Bound::Unbounded,
            };
            let mut got = Vec::new();
            for item in kv
                .scan(Bound::Included(&[first]), borrow(&to))
                .map_err(|e| e.to_string())?
            {
                got.push(item.map_err(|e| e.to_string())?);
            }
            let want: Vec<_> = mine.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            first_difference(&got, &want)
                .map_err(|e| format!("its own keys after batch {i}: {e}"))?;
        }
    }
    Ok(mine)
}
