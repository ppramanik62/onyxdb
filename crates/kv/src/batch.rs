//! `Batch`: a list of puts and deletes that an engine applies whole or not
//! at all, and its encoding in bytes.
//!
//! A batch is the only way to write. A single put is a batch with one op.
//! The ops run in the order they were added, so a put and then a delete of
//! the same key leaves the key deleted.
//!
//! The batch checks every key and value when it takes them. A key longer
//! than [`MAX_KEY`](crate::MAX_KEY) or a value longer than
//! [`MAX_VALUE`](crate::MAX_VALUE) is refused at that point, so no engine
//! ever recieves a batch that breaks the limits, and no engine has to repeat
//! the check.
//!
//! An engine that keeps its batches in a log, stores each one as the bytes
//! from [`Batch::encode`]. The bytes are the ops, one after another, with no
//! count in front: the length of the log record ends the batch. All numbers
//! are little-endian.
//!
//! ```text
//! put     0      1        3          3+k       7+k
//!         ┌──────┬────────┬──────────┬─────────┬───────────┐
//!         │ 1    │ k: u16 │ key (k)  │ v: u32  │ value (v) │
//!         └──────┴────────┴──────────┴─────────┴───────────┘
//! delete  0      1        3          3+k
//!         ┌──────┬────────┬──────────┐
//!         │ 2    │ k: u16 │ key (k)  │
//!         └──────┴────────┴──────────┘
//! ```
//!
//! A key fits in a `u16` length because [`MAX_KEY`](crate::MAX_KEY) is 512,
//! and a value fits in a `u32` length because [`MAX_VALUE`](crate::MAX_VALUE)
//! is 1 MiB.

use crate::{Error, Result};

/// A list of puts and deletes, applied in order, whole or not at all.
///
/// # Examples
///
/// ```
/// use kv::{Batch, Op};
///
/// let mut batch = Batch::new();
/// batch.put("apple", "red").unwrap();
/// batch.delete("pear").unwrap();
/// assert_eq!(batch.len(), 2);
/// assert_eq!(batch.ops()[1], Op::Delete(b"pear".to_vec()));
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Batch {
    /// The ops, in the order they were added. Every key and value in them
    /// is within the limits. The suite in this crate edits the list
    /// directly when it shrinks a failing test.
    pub(crate) ops: Vec<Op>,
}

/// One change in a [`Batch`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// Set the key to the value. An empty value is a value: the key exists
    /// afterwards, and `get` returns `Some` with no bytes in it.
    Put(Vec<u8>, Vec<u8>),
    /// Remove the key. Deleting a key that does not exist does nothing.
    Delete(Vec<u8>),
}

impl Batch {
    /// Makes an empty batch.
    ///
    /// An empty batch writes nothing. Applied with `sync`, it makes every
    /// batch durable those were applied before it.
    ///
    /// # Examples
    ///
    /// ```
    /// use kv::Batch;
    ///
    /// assert!(Batch::new().is_empty());
    /// ```
    pub fn new() -> Batch {
        Batch::default()
    }

    /// Adds a put of `key` with `value`.
    ///
    /// `impl Into<Vec<u8>>` accepts a `Vec<u8>`, which the batch keeps
    /// without copying, and also a `&[u8]`, a `&str` or a `String`.
    ///
    /// # Errors
    ///
    /// [`Error::KeyTooLong`] if `key` is longer than [`MAX_KEY`](crate::MAX_KEY), and
    /// [`Error::ValueTooLong`] if `value` is longer than [`MAX_VALUE`](crate::MAX_VALUE). The
    /// batch is unchanged then.
    ///
    /// # Examples
    ///
    /// ```
    /// use kv::{Batch, MAX_KEY};
    ///
    /// let mut batch = Batch::new();
    /// batch.put(vec![b'k'; MAX_KEY], "").unwrap();
    /// assert!(batch.put(vec![b'k'; MAX_KEY + 1], "").is_err());
    /// assert_eq!(batch.len(), 1);
    /// ```
    pub fn put(&mut self, key: impl Into<Vec<u8>>, value: impl Into<Vec<u8>>) -> Result<()> {
        let (key, value) = (key.into(), value.into());
        Error::check_key(&key)?;
        Error::check_value(&value)?;
        self.ops.push(Op::Put(key, value));
        Ok(())
    }

    /// Adds a delete of `key`.
    ///
    /// # Errors
    ///
    /// [`Error::KeyTooLong`] if `key` is longer than [`MAX_KEY`](crate::MAX_KEY). No such
    /// key can exist, but the limit holds for every key in a batch, so the
    /// encoding can always store it.
    ///
    /// # Examples
    ///
    /// ```
    /// use kv::Batch;
    ///
    /// let mut batch = Batch::new();
    /// batch.delete(b"apple").unwrap();
    /// assert_eq!(batch.len(), 1);
    /// ```
    pub fn delete(&mut self, key: impl Into<Vec<u8>>) -> Result<()> {
        let key = key.into();
        Error::check_key(&key)?;
        self.ops.push(Op::Delete(key));
        Ok(())
    }

    /// The ops, in the order they were added. An engine applies them in
    /// this order.
    ///
    /// # Examples
    ///
    /// ```
    /// use kv::{Batch, Op};
    ///
    /// let mut batch = Batch::new();
    /// batch.put("a", "1").unwrap();
    /// for op in batch.ops() {
    ///     match op {
    ///         Op::Put(key, value) => assert_eq!((&key[..], &value[..]), (&b"a"[..], &b"1"[..])),
    ///         Op::Delete(_) => unreachable!(),
    ///     }
    /// }
    /// ```
    pub fn ops(&self) -> &[Op] {
        &self.ops
    }

    /// How many ops the batch holds.
    pub fn len(&self) -> usize {
        self.ops.len()
    }

    /// Whether the batch holds no ops.
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }
}

/// The first byte of an encoded put.
const PUT: u8 = 1;
/// The first byte of an encoded delete.
const DELETE: u8 = 2;

impl Batch {
    /// Encodes the batch as bytes, in the format the module header draws.
    /// An empty batch encodes as no bytes.
    ///
    /// # Examples
    ///
    /// ```
    /// use kv::Batch;
    ///
    /// let mut batch = Batch::new();
    /// batch.put("k", "vv").unwrap();
    /// batch.delete("k").unwrap();
    /// assert_eq!(batch.encode(), b"\x01\x01\x00k\x02\x00\x00\x00vv\x02\x01\x00k");
    /// assert_eq!(Batch::decode(&batch.encode()).unwrap(), batch);
    /// ```
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for op in &self.ops {
            // The limiits make these casts exact: a key is at most 512 bytes
            // and a value at most 1 MiB.
            match op {
                Op::Put(key, value) => {
                    out.push(PUT);
                    out.extend_from_slice(&(key.len() as u16).to_le_bytes());
                    out.extend_from_slice(key);
                    out.extend_from_slice(&(value.len() as u32).to_le_bytes());
                    out.extend_from_slice(value);
                }
                Op::Delete(key) => {
                    out.push(DELETE);
                    out.extend_from_slice(&(key.len() as u16).to_le_bytes());
                    out.extend_from_slice(key);
                }
            }
        }
        out
    }

    /// Decodes bytes that [`encode`](Batch::encode) made.
    ///
    /// # Errors
    ///
    /// [`Error::Corrupt`] if the bytes break the format: an op kind other
    /// than 1 or 2, a length that runs past the end of the bytes, or a key
    /// or value longer than its limit. The message gives the byte offset.
    /// The decoder never panics, whatever the bytes are.
    ///
    /// # Examples
    ///
    /// ```
    /// use kv::{Batch, Error};
    ///
    /// assert!(Batch::decode(b"").unwrap().is_empty());
    /// // A put whose key length says 5, with only 1 byte after it.
    /// let err = Batch::decode(b"\x01\x05\x00k").unwrap_err();
    /// assert!(matches!(err, Error::Corrupt(_)));
    /// ```
    pub fn decode(bytes: &[u8]) -> Result<Batch> {
        let mut batch = Batch::new();
        let mut at = 0;
        while at < bytes.len() {
            let start = at;
            let kind = take(bytes, &mut at, 1)?[0];
            if kind != PUT && kind != DELETE {
                return Err(Error::Corrupt(format!(
                    "batch: op at byte {start} has kind {kind}, which is neither 1 (put) nor 2 (delete)"
                )));
            }
            let len = u16::from_le_bytes(take(bytes, &mut at, 2)?.try_into().unwrap());
            let key = take(bytes, &mut at, len as usize)?.to_vec();
            // A key or value over the limit means the bytes did not come
            // from `encode`, so it is corruption, not bad input.
            let corrupt = |e: Error| Error::Corrupt(format!("batch: op at byte {start}: {e}"));
            if kind == PUT {
                let len = u32::from_le_bytes(take(bytes, &mut at, 4)?.try_into().unwrap());
                let value = take(bytes, &mut at, len as usize)?.to_vec();
                batch.put(key, value).map_err(corrupt)?;
            } else {
                batch.delete(key).map_err(corrupt)?;
            }
        }
        Ok(batch)
    }
}

/// Returns the `n` bytes at `*at` and moves `*at` past them, or an error if
/// fewer than `n` bytes are left.
fn take<'a>(bytes: &'a [u8], at: &mut usize, n: usize) -> Result<&'a [u8]> {
    match bytes.get(*at..).and_then(|rest| rest.get(..n)) {
        Some(taken) => {
            *at += n;
            Ok(taken)
        }
        None => Err(Error::Corrupt(format!(
            "batch: {n} bytes needed at byte {}, but the batch is {} bytes long",
            *at,
            bytes.len()
        ))),
    }
}
