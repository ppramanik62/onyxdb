//! Prints the different keys among 1,000 random ones, in the order a scan of the model returns them,
//! one per line in hex, so another program can sort the same keys and
//! compare.
//!
//!     cargo run --example order

use std::{ops::Bound, sync::Arc};

use kv::{Batch, Kv, Model};
use vfs::{Mem, Rng};

fn main() {
    let model = Model::create(Arc::new(Mem::new()), "kv").unwrap();
    let mut rng = Rng::new(5);
    let mut batch = Batch::new();
    for _ in 0..1000 {
        // Keys of 0 to 4 bytes, with the bytes where comparison bugs hide.
        let len = rng.below(5);
        let key: Vec<u8> = (0..len)
            .map(|_| [0x00, 0x01, b'a', 0x7f, 0x80, 0xff][rng.below(6)])
            .collect();
        batch.put(key, "").unwrap();
    }
    model.apply(batch, false).unwrap();
    for item in model.scan(Bound::Unbounded, Bound::Unbounded).unwrap() {
        let (key, _) = item.unwrap();
        let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
        println!("{hex}");
    }
}
