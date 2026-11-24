//! What 100 crashes leave of records that were not synced.
//!
//!     cargo run --example torn_log

use std::{collections::BTreeMap, sync::Arc};

use vfs::Mem;
use wal::{BLOCK, Wal};

fn main() {
    // How many crashes left each list of records.
    let mut outcomes: BTreeMap<String, usize> = BTreeMap::new();
    for seed in 0..100 {
        let mem = Mem::new();
        let wal = Wal::create(Arc::new(mem.clone()), "wal", BLOCK as u64).unwrap();
        // Records 0 to 2 are synced. Records 3 and 4 are only written.
        for (i, len) in [300, 5000, 300, 100, 400].into_iter().enumerate() {
            wal.append(&vec![i as u8; len]).unwrap();
            if i == 2 {
                wal.flush_to(u64::MAX).unwrap();
            }
        }
        if seed == 0 {
            println!("records 0 to 2 synced, ending at LSN {}", wal.durable());
            println!("records 3 and 4 written, ending at LSN {}", wal.end());
            println!();
        }
        mem.crash(seed);
        let wal = Wal::open(Arc::new(mem), "wal", BLOCK as u64).unwrap();
        let records: Vec<String> = wal
            .replay()
            .unwrap()
            .map(|r| r.unwrap().data[0].to_string())
            .collect();
        let outcome = format!("replay {:<10} end {}", records.join(" "), wal.end());
        *outcomes.entry(outcome).or_default() += 1;
    }
    for (outcome, crashes) in outcomes {
        println!("{crashes:>3} crashes: {outcome}");
    }
}
