//! `Rng`: a small random number generator (xorshift64*).
//!
//! The same seed always gives the same numbers, so a crash chosen from a seed
//! can be repeated exactly. [`seeds`] gives every randomized test its seeds,
//! so one setting can make all of them run longer.

use std::{env, ops::Range};

/// A seeded random number generator.
///
/// It is fast and small, and its numbers are good enough to choose crashes
/// and test inputs. It is not for anything that needs to be hard to guess.
///
/// # Examples
///
/// ```
/// use vfs::Rng;
///
/// let (mut a, mut b) = (Rng::new(7), Rng::new(7));
/// assert_eq!(a.next_u64(), b.next_u64());
/// assert!(a.below(10) < 10);
/// assert_eq!(a.bytes(3).len(), 3);
/// ```
pub struct Rng(u64);

impl Rng {
    /// Makes a generator started from `seed`. Any seed works, 0 included,
    /// and the same seed always gives the same numbers.
    pub fn new(seed: u64) -> Rng {
        // xorshift returns only zeros if its state is 0, and small seeds give
        // similar first numbers. Mixing the seed with an odd constant fixes
        // both problems.
        let state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03;
        Rng(if state == 0 { 1 } else { state })
    }

    /// The next 64 random bits, and moves the generator on.
    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// A number from 0 up to, but not including, `n`.
    ///
    /// It takes the remainder of a 64-bit number, so small numbers are very
    /// slightly more likely. For an `n` far below 2^64, that does not matter.
    ///
    /// # Panics
    ///
    /// Panics if `n` is 0.
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }

    /// Makes `len` random bytes.
    pub fn bytes(&mut self, len: usize) -> Vec<u8> {
        (0..len).map(|_| self.next_u64() as u8).collect()
    }
}

/// The environment variable that multiplies every seed count.
const SEEDS_VAR: &str = "ONYXDB_SEEDS";

/// The seeds a randomized test runs: `0..n`, times the whole number in the
/// `ONYXDB_SEEDS` environment variable when it is set.
///
/// Every randomized test takes its seed count from here, and so does
/// [`crash_test`](crate::crash_test). One setting then runs all of them
/// longer, for example `ONYXDB_SEEDS=10 cargo test` overnight, while a plain
/// `cargo test` keeps the counts the tests chose.
///
/// # Panics
///
/// Panics if `ONYXDB_SEEDS` is set to anything other than a whole number of
/// at least 1. A test that ran its usual seeds after a typing mistake would
/// look like a longer run that passed.
///
/// # Examples
///
/// ```
/// // 20 seeds, or a multiple of 20 when ONYXDB_SEEDS is set.
/// let seeds = vfs::seeds(20);
/// assert_eq!(seeds.start, 0);
/// assert_eq!(seeds.end % 20, 0);
/// ```
pub fn seeds(n: u64) -> Range<u64> {
    let factor = match env::var(SEEDS_VAR) {
        Err(env::VarError::NotPresent) => 1,
        Ok(value) => match value.parse::<u64>() {
            Ok(factor) if factor >= 1 => factor,
            _ => panic!("{SEEDS_VAR} is {value:?}, but it must be a whole number of at least 1"),
        },
        Err(env::VarError::NotUnicode(value)) => {
            panic!("{SEEDS_VAR} is {value:?}, but it must be a whole number of at least 1")
        }
    };
    0..n.saturating_mul(factor)
}
