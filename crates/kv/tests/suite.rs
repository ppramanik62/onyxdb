//! The suite on the model: random steps against a `BTreeMap`, and threads.

mod common;

use common::ModelEngine;
use kv::suite::{Step, generate, run, threads};

#[test]
fn the_model_passes() {
    let mut steps = 0;
    for seed in vfs::seeds(100) {
        let generated = generate(seed, 300);
        run(&ModelEngine, &generated).unwrap_or_else(|e| panic!("seed {seed}: {e}"));
        steps += generated.len();
    }
    println!("100 seeds, {steps} steps: every answer matched");
}

#[test]
fn the_generator_reaches_the_edges() {
    let steps: Vec<Step> = (0..100).flat_map(|seed| generate(seed, 300)).collect();
    let count = |f: &dyn Fn(&Step) -> bool| steps.iter().filter(|s| f(s)).count();
    println!("applies:      {}", count(&|s| matches!(s, Step::Apply(..))));
    println!("gets:         {}", count(&|s| matches!(s, Step::Get(_))));
    println!("scans:        {}", count(&|s| matches!(s, Step::Scan(..))));
    println!(
        "scans while writing: {}",
        count(&|s| matches!(s, Step::ScanWhile { .. }))
    );
    println!("reopens:      {}", count(&|s| matches!(s, Step::Reopen)));
    let backward = count(&|s| match s {
        Step::Scan(from, to) => kv::empty_range(
            from.as_ref().map(Vec::as_slice),
            to.as_ref().map(Vec::as_slice),
        ),
        _ => false,
    });
    println!("empty or backward scans: {backward}");
    assert!(backward > 0);
}

#[test]
fn the_model_passes_with_threads() {
    threads(&ModelEngine, 8, 500).unwrap();
    println!("8 threads, 500 batches each: every answer matched");
}
