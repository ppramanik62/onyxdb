//! The model after a crash at every write and after every failed sync, and
//! an engine that applies a batch in parts.

mod common;

use common::{ModelEngine, SplitEngine};
use kv::suite::{Step, check, check_crashes, crashes, generate};

#[test]
fn the_model_survives_crashes() {
    let mut checked = 0;
    for seed in 0..20 {
        checked += crashes(&ModelEngine, &generate(seed, 30), 4)
            .unwrap_or_else(|e| panic!("seed {seed}: {e}"));
    }
    println!("20 seeds of 30 steps: {checked} crashes, every one left a prefix");
}

#[test]
fn a_batch_in_parts_passes_without_crashes() {
    check(&SplitEngine, 100, 300).unwrap();
}

#[test]
fn a_batch_in_parts_fails_with_crashes() {
    let failure = check_crashes(&SplitEngine, 20, 30).unwrap_err();
    println!("{failure}");
    let [Step::Apply(batch, _)] = &failure.steps[..] else {
        panic!("{failure}")
    };
    assert_eq!(batch.len(), 2, "{failure}");
}
