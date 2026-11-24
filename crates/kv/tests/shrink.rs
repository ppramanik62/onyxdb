//! `check` on the model, and on an engine whose scans skip their last key.

mod common;

use common::{ModelEngine, SkipLastEngine};
use kv::suite::check;

#[test]
fn the_model_passes_check() {
    check(&ModelEngine, 100, 300).unwrap();
}

#[test]
fn a_scan_that_skips_its_last_key_fails() {
    let failure = check(&SkipLastEngine, 100, 300).unwrap_err();
    println!("{failure}");
    assert!(failure.steps.len() <= 3, "{failure}");
}
