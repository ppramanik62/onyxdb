//! The model engine: the promises of `Kv`, one at a time.

use std::{
    ops::Bound::{self, Excluded, Included, Unbounded},
    sync::Arc,
};

use kv::{Batch, Kv, Model};
use vfs::Mem;

/// A batch of puts, from pairs of strings.
fn puts(pairs: &[(&str, &str)]) -> Batch {
    let mut batch = Batch::new();
    for (key, value) in pairs {
        batch.put(*key, *value).unwrap();
    }
    batch
}

/// Every key a scan returns, as strings.
fn keys(kv: &Model, from: Bound<&[u8]>, to: Bound<&[u8]>) -> Vec<String> {
    kv.scan(from, to)
        .unwrap()
        .map(|item| String::from_utf8(item.unwrap().0).unwrap())
        .collect()
}

#[test]
fn empty_value_is_a_value() {
    let kv = Model::create(Arc::new(Mem::new()), "kv").unwrap();
    kv.apply(puts(&[("a", "")]), false).unwrap();
    assert_eq!(kv.get(b"a").unwrap(), Some(Vec::new()));
    assert_eq!(kv.get(b"b").unwrap(), None);
}

#[test]
fn ops_run_in_order() {
    let kv = Model::create(Arc::new(Mem::new()), "kv").unwrap();
    let mut batch = puts(&[("a", "1")]);
    batch.delete("a").unwrap();
    batch.put("b", "1").unwrap();
    batch.put("b", "2").unwrap();
    kv.apply(batch, false).unwrap();
    assert_eq!(kv.get(b"a").unwrap(), None);
    assert_eq!(kv.get(b"b").unwrap(), Some(b"2".to_vec()));
}

#[test]
fn scans_and_bounds() {
    let kv = Model::create(Arc::new(Mem::new()), "kv").unwrap();
    kv.apply(
        puts(&[("a", "1"), ("b", "2"), ("ba", "3"), ("c", "4")]),
        false,
    )
    .unwrap();
    assert_eq!(keys(&kv, Unbounded, Unbounded), ["a", "b", "ba", "c"]);
    assert_eq!(keys(&kv, Included(b"b"), Excluded(b"c")), ["b", "ba"]);
    assert_eq!(keys(&kv, Excluded(b"b"), Included(b"c")), ["ba", "c"]);
    // Backward and empty ranges return nothing, and do not panic.
    assert!(keys(&kv, Included(b"c"), Included(b"a")).is_empty());
    assert!(keys(&kv, Excluded(b"b"), Excluded(b"b")).is_empty());
}

#[test]
fn write_while_scanning() {
    let kv = Model::create(Arc::new(Mem::new()), "kv").unwrap();
    kv.apply(puts(&[("a", "1"), ("c", "3"), ("e", "5")]), false)
        .unwrap();
    let mut scan = kv.scan(Unbounded, Unbounded).unwrap();
    assert_eq!(scan.next().unwrap().unwrap().0, b"a");
    // The scan holds no lock, so the same thread can write. A key ahead
    // of the scan is seen, and a deleted one is not.
    let mut batch = puts(&[("d", "4")]);
    batch.delete("e").unwrap();
    kv.apply(batch, false).unwrap();
    let rest: Vec<_> = scan.map(|item| item.unwrap().0).collect();
    assert_eq!(rest, [b"c".to_vec(), b"d".to_vec()]);
}

#[test]
fn reopen_keeps_every_batch() {
    let mem = Arc::new(Mem::new());
    let kv = Model::create(mem.clone(), "kv").unwrap();
    kv.apply(puts(&[("a", "1")]), true).unwrap();
    kv.apply(puts(&[("b", "2")]), false).unwrap();
    drop(kv);
    // No crash, so the batch that was not synced is still there.
    let kv = Model::open(mem, "kv").unwrap();
    assert_eq!(keys(&kv, Unbounded, Unbounded), ["a", "b"]);
}

#[test]
fn a_crash_keeps_a_prefix() {
    for seed in vfs::seeds(50) {
        let mem = Mem::new();
        let kv = Model::create(Arc::new(mem.clone()), "kv").unwrap();
        kv.apply(puts(&[("a", "1"), ("b", "1")]), false).unwrap();
        kv.apply(puts(&[("a", "2"), ("b", "2")]), true).unwrap();
        kv.apply(puts(&[("a", "3"), ("b", "3")]), false).unwrap();
        mem.crash(seed);
        let kv = Model::open(Arc::new(mem), "kv").unwrap();
        let a = kv.get(b"a").unwrap().unwrap();
        // The synced batch survives, the last one may not, and no batch
        // survives in part.
        assert!(a == b"2" || a == b"3", "seed {seed}");
        assert_eq!(kv.get(b"b").unwrap().unwrap(), a);
    }
}

#[test]
fn an_empty_batch_syncs() {
    let mem = Mem::new();
    let kv = Model::create(Arc::new(mem.clone()), "kv").unwrap();
    kv.apply(puts(&[("a", "1")]), false).unwrap();
    kv.apply(Batch::new(), true).unwrap();
    mem.crash(0);
    let kv = Model::open(Arc::new(mem), "kv").unwrap();
    assert_eq!(kv.get(b"a").unwrap(), Some(b"1".to_vec()));
}
