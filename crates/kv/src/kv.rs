//! The [`Kv`] trait, and the promises every engine makes behind it.
//!
//! The code above storage sees three operations. `get` reads one key,
//! `scan` reads a range of keys in order, and `apply` writes a batch. The
//! promises below are the whole contract. The suite in this crate checks
//! each of them, and code above storage may rely on these and on nothing
//! else.
//!
//! | Promise | Exactly |
//! |---|---|
//! | order | Keys compare as byte strings: byte by byte, and a key that is a prefix of another comes first. A scan returns keys in strictly increasing order, all inside its bounds. |
//! | empty values | An empty value is a value. `get` returns `Some` with no bytes, not `None`. |
//! | empty ranges | A scan whose start is after its end, or whose start equals its end with either bound excluded, returns nothing. |
//! | visible | Once `apply` returns `Ok`, every later `get` and `scan` sees the whole batch. |
//! | reopen | With no crash, dropping the engine and opening it again keeps every applied batch, synced or not. |
//! | crash | After a crash, the engine holds exactly the batches of some prefix of the applied batches, in order. The prefix includes every batch up to the last one applied with `sync`. No batch survives in part. |
//! | open | What an engine holds just after it opens survives a later crash. |
//! | errors | If `apply` fails, its batch may survive or not, but never in part. The engine may refuse every later write with `Stopped` until it is opened again. |
//! | scans during writes | A scan does not see a snapshot. It sees each key as it is when the scan reaches it. A key that no batch changes while the scan runs is returned with its value. A key that a batch changes may come back with its old value or its new one, or not at all if the batch deletes it. |
//! | threads | Batches from many threads are applied one after another. A reader in another thread may see part of a batch while it is applied, but never part of one key's value. |
//!
//! The last two promises are weaker than a snapshot. They are what an
//! engine that changes pages in place can keep without copying pages or
//! blocking writers. Code that needs a consistent view of many keys builds
//! it above this trait, with versioned keys.

use std::ops::Bound;

use crate::{Batch, Result};

/// An ordered store of byte keys and byte values: the interface every
/// storage engine implements.
///
/// The module header lists every promise an engine makes. `Kv` is `Send`
/// and `Sync`, so threads can share one engine through an `Arc` or a
/// reference. The trait works as `dyn Kv`, so a program can choose its
/// engine while it runs.
///
/// # Examples
///
/// An engine that keeps everything in memory. It loses its data at a crash,
/// so it breaks the crash promise, but it shows what each method must do.
///
/// ```
/// use std::collections::BTreeMap;
/// use std::ops::Bound;
/// use std::sync::Mutex;
/// use kv::{Batch, Iter, Kv, Op, Result, empty_range};
///
/// struct Memory(Mutex<BTreeMap<Vec<u8>, Vec<u8>>>);
///
/// impl Kv for Memory {
///     fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>> {
///         Ok(self.0.lock().unwrap().get(key).cloned())
///     }
///
///     fn scan(&self, from: Bound<&[u8]>, to: Bound<&[u8]>) -> Result<Iter<'_>> {
///         if empty_range(from, to) {
///             return Ok(Box::new(std::iter::empty()));
///         }
///         // Copies the range, so the scan holds no lock.
///         let map = self.0.lock().unwrap();
///         let items: Vec<_> = map.range::<[u8], _>((from, to)).map(|(k, v)| Ok((k.clone(), v.clone()))).collect();
///         Ok(Box::new(items.into_iter()))
///     }
///
///     fn apply(&self, batch: Batch, _sync: bool) -> Result<()> {
///         let mut map = self.0.lock().unwrap();
///         for op in batch.ops() {
///             match op {
///                 Op::Put(key, value) => map.insert(key.clone(), value.clone()),
///                 Op::Delete(key) => map.remove(key),
///             };
///         }
///         Ok(())
///     }
/// }
///
/// let kv = Memory(Mutex::new(BTreeMap::new()));
/// let mut batch = Batch::new();
/// batch.put("b", "2").unwrap();
/// batch.put("a", "1").unwrap();
/// kv.apply(batch, true).unwrap();
/// assert_eq!(kv.get(b"a").unwrap(), Some(b"1".to_vec()));
/// let keys: Vec<_> = kv.scan(Bound::Unbounded, Bound::Unbounded).unwrap().map(|r| r.unwrap().0).collect();
/// assert_eq!(keys, [b"a".to_vec(), b"b".to_vec()]);
/// ```
pub trait Kv: Send + Sync {
    /// Returns the value of `key`, or `None` if the key does not exist.
    ///
    /// # Errors
    ///
    /// Any error the engine had while it read, such as [`Error::Vfs`] or
    /// [`Error::Corrupt`].
    ///
    /// [`Error::Vfs`]: crate::Error::Vfs
    /// [`Error::Corrupt`]: crate::Error::Corrupt
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>>;

    /// Returns the keys from `from` to `to`, in order, with their values.
    ///
    /// `Bound` comes from `std::ops`: `Included(k)`, `Excluded(k)` or
    /// `Unbounded`. A range that holds no key, such as a start after the
    /// end, gives an empty scan. The scan sees each key as it is when it
    /// reaches the key, and it holds nothing that stops a write, even a
    /// write from the same thread.
    ///
    /// # Errors
    ///
    /// Any error the engine had while it started the scan. An error while
    /// the scan runs comes back as an `Err` item, and the scan ends after
    /// it.
    fn scan(&self, from: Bound<&[u8]>, to: Bound<&[u8]>) -> Result<Iter<'_>>;

    /// Applies every op in `batch`, in order, whole or not at all. With
    /// `sync`, it returns only once this batch and every batch applied
    /// before it survive a crash.
    ///
    /// # Errors
    ///
    /// Any error the engine had while it wrote or synced. The batch may
    /// survive or not, but never in part. After an error, the engine may
    /// return [`Error::Stopped`] for every write until it is opened again.
    ///
    /// [`Error::Stopped`]: crate::Error::Stopped
    fn apply(&self, batch: Batch, sync: bool) -> Result<()>;
}

/// What [`Kv::scan`] returns: an iterator of key and value pairs, in key
/// order.
///
/// It is boxed so that every engine can return its own iterator type
/// through `dyn Kv`. The lifetime `'a` ties it to the engine, so a scan
/// cannot be used after the engine it reads is dropped.
pub type Iter<'a> = Box<dyn Iterator<Item = Result<(Vec<u8>, Vec<u8>)>> + 'a>;

/// Whether the range from `from` to `to` can hold no key at all: its start
/// is after its end, or its start equals its end and either bound excludes
/// it.
///
/// A scan of such a range must return nothing. `BTreeMap::range` panics on
/// the first case, and on the second when both bounds exclude the key, so
/// an engine checks this first.
///
/// # Examples
///
/// ```
/// use std::ops::Bound::{Excluded, Included, Unbounded};
/// use kv::empty_range;
///
/// assert!(empty_range(Included(b"b"), Included(b"a")));
/// assert!(empty_range(Included(b"a"), Excluded(b"a")));
/// assert!(!empty_range(Included(b"a"), Included(b"a")));
/// assert!(!empty_range(Unbounded, Excluded(b"")));
/// ```
pub fn empty_range(from: Bound<&[u8]>, to: Bound<&[u8]>) -> bool {
    use Bound::{Excluded, Included, Unbounded};
    match (from, to) {
        (Unbounded, _) | (_, Unbounded) => false,
        (Included(a), Included(b)) => a > b,
        (Included(a) | Excluded(a), Excluded(b)) | (Excluded(a), Included(b)) => a >= b,
    }
}
