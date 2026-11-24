//! `generate`: random steps from a seed, aimed at the edges where engines
//! go wrong.
//!
//! Most keys come from a small alphabet and are 0 to 3 bytes long, so the
//! same keys come back again and again: a get usually finds a key, a delete
//! usually removes one, and ranges overlap. The alphabet holds the byte
//! values where comparison bugs hide:
//!
//! | Byte | Why |
//! |---|---|
//! | `0x00` | the smallest byte, and the end of a C string |
//! | `0x01`, `a`, `b` | ordinary neighbors |
//! | `0x7f`, `0x80` | the edge where a signed byte turns negative |
//! | `0xff` | the largest byte |
//!
//! The generator also makes, on purpose: the empty key, keys of exactly
//! `MAX_KEY` bytes, empty values, values bigger than a 4 KiB page, empty
//! batches, a put and a delete of the same key in one batch, deletes of
//! missing keys, and ranges whose start is after their end.

use std::ops::Bound;

use vfs::Rng;

use crate::{Batch, MAX_KEY};

use super::Step;

/// The bytes that most keys are made of.
const ALPHABET: [u8; 7] = [0x00, 0x01, b'a', b'b', 0x7f, 0x80, 0xff];

/// Makes `len` random steps from `seed`. The same seed and length always
/// give the same steps.
///
/// About half the steps apply a batch, and a third of those batches use
/// `sync`. The rest are gets, scans, scans with writes during them, and a
/// few reopens.
///
/// # Examples
///
/// ```
/// use kv::suite::{Step, generate};
///
/// let steps = generate(7, 100);
/// assert_eq!(steps, generate(7, 100));
/// assert!(steps.iter().any(|s| matches!(s, Step::Apply(..))));
/// ```
pub fn generate(seed: u64, len: usize) -> Vec<Step> {
    let mut rng = Rng::new(seed);
    (0..len).map(|_| step(&mut rng)).collect()
}

/// Makes one random step.
fn step(rng: &mut Rng) -> Step {
    match rng.below(100) {
        0..=46 => Step::Apply(batch(rng), rng.below(3) == 0),
        47..=66 => Step::Get(key(rng)),
        67..=84 => Step::Scan(bound(rng), bound(rng)),
        85..=96 => Step::ScanWhile {
            from: bound(rng),
            to: bound(rng),
            writes: (0..1 + rng.below(3)).map(|_| batch(rng)).collect(),
        },
        _ => Step::Reopen,
    }
}

/// Makes a batch of 0 to 4 ops. Seven ops in ten are puts.
pub(crate) fn batch(rng: &mut Rng) -> Batch {
    let mut batch = Batch::new();
    for _ in 0..rng.below(5) {
        // Every key and value here is within the limits, so the batch
        // always takes them.
        if rng.below(10) < 7 {
            batch.put(key(rng), value(rng)).unwrap();
        } else {
            batch.delete(key(rng)).unwrap();
        }
    }
    batch
}

/// Makes a key. One key in 40 is exactly `MAX_KEY` bytes long, and the
/// rest come from [`short_key`].
fn key(rng: &mut Rng) -> Vec<u8> {
    if rng.below(40) == 0 {
        let mut key = vec![b'a'; MAX_KEY];
        key[MAX_KEY - 1] = ALPHABET[rng.below(ALPHABET.len())];
        return key;
    }
    short_key(rng)
}

/// Makes a key of 0 to 3 bytes from the alphabet. Lengths 1 and 2 are the
/// most common, so keys repeat often.
pub(crate) fn short_key(rng: &mut Rng) -> Vec<u8> {
    let len = [0, 1, 1, 2, 2, 2, 3][rng.below(7)];
    (0..len)
        .map(|_| ALPHABET[rng.below(ALPHABET.len())])
        .collect()
}

/// Makes a value: a quarter are empty, most are 1 to 8 random bytes, and
/// one in 20 is 5,000 to 20,000 bytes long.
pub(crate) fn value(rng: &mut Rng) -> Vec<u8> {
    let len = match rng.below(20) {
        0..=4 => 0,
        5..=18 => 1 + rng.below(8),
        _ => 5_000 + rng.below(15_000),
    };
    rng.bytes(len)
}

/// Makes one bound of a range. One in five is `Unbounded`.
fn bound(rng: &mut Rng) -> Bound<Vec<u8>> {
    match rng.below(5) {
        0 => Bound::Unbounded,
        1 | 2 => Bound::Included(key(rng)),
        _ => Bound::Excluded(key(rng)),
    }
}
