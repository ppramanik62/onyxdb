//! `Model`: the model engine, a `BTreeMap` in memory and a log of batches
//! on disk.
//!
//! The model is the engine that is easiest to believe. Every key lives in a
//! `BTreeMap`, which keeps keys in byte order. Every batch is also one
//! record in a `wal` log, so the batches survive a crash. Opening the model
//! replays the whole log into an empty map.
//!
//! ```text
//!  apply(batch, sync)
//!    1. take the writer mutex
//!    2. append batch.encode() to the log      the log record is the commit
//!    3. if sync: flush the log                the batch is durable
//!    4. write-lock the map, apply the ops     readers see the batch
//!
//!  open: replay every record of the log into an empty map
//! ```
//!
//! The log decides what a crash keeps. It returns whole records, in order,
//! and every record up to the last flush, so the model keeps a prefix of
//! the batches, each one whole. The model never removes old records, so
//! opening it reads every batch it ever applied. That is fine for tests and
//! wrong for real data.
//!
//! Two locks keep the model correct with many threads. The writer mutex
//! lets one `apply` run at a time, so batches reach the log and the map in
//! the same order. Without it, two batches that write the same key could
//! reach the map in one order and the log in the other, and opening the
//! model again would give a different value than readers saw. The map's
//! `RwLock` is held only while the map changes, so readers continue while
//! a writer waits for a sync.

use std::{
    collections::BTreeMap,
    ops::Bound,
    path::Path,
    sync::{Arc, Mutex, RwLock, RwLockReadGuard},
};

use vfs::Vfs;
use wal::{BLOCK, Wal};

use crate::{Batch, Iter, Kv, Op, Result, empty_range};

/// The size of each log segment: 1 MiB, a multiple of the log's block.
const SEGMENT: u64 = 32 * BLOCK as u64;

/// The model engine: a `BTreeMap` in memory and a log of batches on disk.
///
/// The model keeps every promise of [`Kv`]. It exists so that tests have an
/// engine to trust: the code above storage runs its own tests on it, and
/// the suite in this crate checks it first.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use kv::{Batch, Kv, Model};
/// use vfs::Mem;
///
/// let mem = Arc::new(Mem::new());
/// let model = Model::create(mem.clone(), "kv").unwrap();
/// let mut batch = Batch::new();
/// batch.put("apple", "red").unwrap();
/// model.apply(batch, true).unwrap();
/// drop(model);
///
/// let model = Model::open(mem, "kv").unwrap();
/// assert_eq!(model.get(b"apple").unwrap(), Some(b"red".to_vec()));
/// ```
pub struct Model {
    /// The log. Each record is one batch, encoded.
    wal: Wal,
    /// Every key and its value, as of the last applied batch.
    map: RwLock<BTreeMap<Vec<u8>, Vec<u8>>>,
    /// Held for the whole of `apply`, so one batch is applied at a time.
    writer: Mutex<()>,
}

impl Model {
    /// Creates an empty model in the directory `dir`, which must not exist
    /// yet.
    ///
    /// # Errors
    ///
    /// [`Error::Vfs`](crate::Error::Vfs) if a file system call fails. If
    /// `dir` already exists, that is `vfs::Error::AlreadyExists`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use kv::Model;
    /// use vfs::Mem;
    ///
    /// let mem = Arc::new(Mem::new());
    /// Model::create(mem.clone(), "kv").unwrap();
    /// assert!(Model::create(mem, "kv").is_err());
    /// ```
    pub fn create(vfs: Arc<dyn Vfs>, dir: impl AsRef<Path>) -> Result<Model> {
        let wal = Wal::create(vfs, dir, SEGMENT)?;
        Ok(Model::new(wal, BTreeMap::new()))
    }

    /// Opens the model in `dir` and replays its log, so the model holds
    /// every batch the log kept.
    ///
    /// Opening the log makes everything it holds durable, so what the model
    /// holds after `open` survives a later crash.
    ///
    /// # Errors
    ///
    /// [`Error::Vfs`](crate::Error::Vfs) if a file system call fails, and
    /// [`Error::Corrupt`](crate::Error::Corrupt) if the log or a batch in it
    /// breaks its format.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use kv::Model;
    /// use vfs::Mem;
    ///
    /// assert!(Model::open(Arc::new(Mem::new()), "kv").is_err());
    /// ```
    pub fn open(vfs: Arc<dyn Vfs>, dir: impl AsRef<Path>) -> Result<Model> {
        let wal = Wal::open(vfs, dir, SEGMENT)?;
        let mut map = BTreeMap::new();
        for record in wal.replay()? {
            apply_ops(&mut map, Batch::decode(&record?.data)?);
        }
        Ok(Model::new(wal, map))
    }

    /// Makes a model from an open log and the map it describes.
    fn new(wal: Wal, map: BTreeMap<Vec<u8>, Vec<u8>>) -> Model {
        Model {
            wal,
            map: RwLock::new(map),
            writer: Mutex::new(()),
        }
    }

    /// Takes the map's read lock.
    ///
    /// # Panics
    ///
    /// If another thread panicked while it held the write lock, which can
    /// only happen in the middle of applying a batch. The map may hold part
    /// of that batch, so no read may use it.
    fn read(&self) -> RwLockReadGuard<'_, BTreeMap<Vec<u8>, Vec<u8>>> {
        self.map
            .read()
            .expect("a thread panicked while it applied a batch")
    }
}

/// Applies the ops of `batch` to `map`, in order.
fn apply_ops(map: &mut BTreeMap<Vec<u8>, Vec<u8>>, batch: Batch) {
    for op in batch.ops {
        match op {
            Op::Put(key, value) => map.insert(key, value),
            Op::Delete(key) => map.remove(&key),
        };
    }
}

/// `get` and `apply` take locks, so they panic if another thread panicked
/// while it applied a batch. `apply` with an empty batch writes nothing to
/// the log. With `sync`, it flushes the log, which makes every earlier
/// batch durable.
impl Kv for Model {
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>> {
        Ok(self.read().get(key).cloned())
    }

    fn apply(&self, batch: Batch, sync: bool) -> Result<()> {
        let _writer = self.writer.lock().expect("a thread panicked in apply");
        let lsn = if batch.is_empty() {
            self.wal.end()
        } else {
            self.wal.append(&batch.encode())?
        };
        if sync {
            self.wal.flush_to(lsn)?;
        }
        // The log holds the batch, so a reader may see it now. If the
        // append or the flush failed, the map stays as it was, and the
        // stopped log refuses every later write.
        let mut map = self.map.write().expect("a thread panicked in apply");
        apply_ops(&mut map, batch);
        Ok(())
    }

    fn scan_range(&self, from: Bound<&[u8]>, to: Bound<&[u8]>) -> Result<Iter<'_>> {
        Ok(Box::new(Scan {
            model: self,
            from: from.map(<[u8]>::to_vec),
            to: to.map(<[u8]>::to_vec),
        }))
    }
}

/// A scan of the model. It holds no lock between items. Each step takes
/// the read lock, finds the first key after the last one it returned, and
/// releases the lock.
///
/// So a writer never waits for a scan, and a thread can write while it
/// scans. The cost is a search for each item, O(log n), instead of a step
/// along the map.
struct Scan<'a> {
    /// The model being scanned.
    model: &'a Model,
    /// Where the next item may start: the scan's own start, and after the
    /// first item, `Excluded` of the last key returned.
    from: Bound<Vec<u8>>,
    /// The scan's end.
    to: Bound<Vec<u8>>,
}

impl Iterator for Scan<'_> {
    type Item = Result<(Vec<u8>, Vec<u8>)>;

    fn next(&mut self) -> Option<Self::Item> {
        let from = self.from.as_ref().map(Vec::as_slice);
        let to = self.to.as_ref().map(Vec::as_slice);
        // `BTreeMap::range` panics on a backward range, and the range
        // shrinks to nothing once the last key is returned.
        if empty_range(from, to) {
            return None;
        }
        let map = self.model.read();
        // The map's keys are `Vec<u8>`, and `range::<[u8], _>` searches
        // them with `&[u8]` bounds, so no bound has to be copied.
        let (key, value) = map.range::<[u8], _>((from, to)).next()?;
        let item = (key.clone(), value.clone());
        drop(map);
        self.from = Bound::Excluded(item.0.clone());
        Some(Ok(item))
    }
}
