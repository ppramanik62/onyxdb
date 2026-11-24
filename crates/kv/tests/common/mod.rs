//! Helpers the tests share: the model as an engine the suite can make, and
//! two engines with a bug planted in them.

// Each test file uses only some of these.
#![allow(dead_code)]

use std::{ops::Bound, sync::Arc};

use kv::{Batch, Iter, Kv, Model, Op, Result, suite::Engine};
use vfs::Vfs;

/// Makes the model in `kv/` for the suite.
pub struct ModelEngine;

impl Engine for ModelEngine {
    type Kv = Model;

    fn create(&self, vfs: Arc<dyn Vfs>) -> Result<Model> {
        Model::create(vfs, "kv")
    }

    fn open(&self, vfs: Arc<dyn Vfs>) -> Result<Model> {
        Model::open(vfs, "kv")
    }
}

/// The model with a bug: every scan leaves out the last key of its range.
pub struct SkipLast(Model);

impl Kv for SkipLast {
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>> {
        self.0.get(key)
    }

    fn scan_range(&self, from: Bound<&[u8]>, to: Bound<&[u8]>) -> Result<Iter<'_>> {
        let mut items: Vec<_> = self.0.scan_range(from, to)?.collect();
        items.pop();
        Ok(Box::new(items.into_iter()))
    }

    fn apply(&self, batch: Batch, sync: bool) -> Result<()> {
        self.0.apply(batch, sync)
    }
}

/// Makes a [`SkipLast`] for the suite.
pub struct SkipLastEngine;

impl Engine for SkipLastEngine {
    type Kv = SkipLast;

    fn create(&self, vfs: Arc<dyn Vfs>) -> Result<SkipLast> {
        Model::create(vfs, "kv").map(SkipLast)
    }

    fn open(&self, vfs: Arc<dyn Vfs>) -> Result<SkipLast> {
        Model::open(vfs, "kv").map(SkipLast)
    }
}

/// The model with a bug: it applies each op of a batch as a batch of its
/// own. Without a crash nobody can tell, because the ops all happen before
/// `apply` returns. A crash between two of them leaves part of the batch.
pub struct Split(Model);

impl Kv for Split {
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>> {
        self.0.get(key)
    }

    fn scan_range(&self, from: Bound<&[u8]>, to: Bound<&[u8]>) -> Result<Iter<'_>> {
        self.0.scan_range(from, to)
    }

    fn apply(&self, batch: Batch, sync: bool) -> Result<()> {
        for op in batch.ops() {
            let mut one = Batch::new();
            match op.clone() {
                Op::Put(key, value) => one.put(key, value)?,
                Op::Delete(key) => one.delete(key)?,
            }
            self.0.apply(one, false)?;
        }
        // An empty batch with `sync` makes all the parts durable.
        self.0.apply(Batch::new(), sync)
    }
}

/// Makes a [`Split`] for the suite.
pub struct SplitEngine;

impl Engine for SplitEngine {
    type Kv = Split;

    fn create(&self, vfs: Arc<dyn Vfs>) -> Result<Split> {
        Model::create(vfs, "kv").map(Split)
    }

    fn open(&self, vfs: Arc<dyn Vfs>) -> Result<Split> {
        Model::open(vfs, "kv").map(Split)
    }
}
