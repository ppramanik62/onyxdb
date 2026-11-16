//! Lays out LevelDB's example records A, B and C, and prints every fragment.
//!
//!     cargo run --example layout

use wal::{
    BLOCK, HEADER,
    format::{decode, encode},
};

fn main() {
    let mut log = Vec::new();
    println!("record  bytes      LSN");
    for (name, len) in [("A", 1000), ("B", 97270), ("C", 8000)] {
        log.extend(encode(log.len() as u64, &vec![b'x'; len]));
        println!("{name}      {len:>6}   {:>6}", log.len());
    }

    println!();
    println!("    at  block  offset  kind     data");
    let mut pos = 0;
    while pos < log.len() {
        let block = pos / BLOCK * BLOCK;
        let left = block + BLOCK - pos;
        if left < HEADER {
            println!(
                "{pos:>6}  {:>5}  {:>6}  trailer  {left} zeros",
                pos / BLOCK,
                pos - block
            );
            pos = block + BLOCK;
            continue;
        }
        let bytes = &log[block..log.len().min(block + BLOCK)];
        let f = decode(bytes, pos - block).unwrap();
        let kind = format!("{:?}", f.kind).to_lowercase();
        println!(
            "{pos:>6}  {:>5}  {:>6}  {kind:<7}  {:>5}",
            pos / BLOCK,
            pos - block,
            f.data.len()
        );
        pos += HEADER + f.data.len();
    }
}
