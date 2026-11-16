//! The hole a failed sync leaves: first in a plain file, then in the log.
//!
//!     cargo run --example hole

use std::{path::Path, sync::Arc};

use vfs::{Fault, File, Mem, Open, Vfs};
use wal::{BLOCK, Wal, format};

fn main() {
    println!("A plain file:");
    let mem = Mem::new();
    let file = mem.open(Path::new("log"), Open::CreateNew).unwrap();
    mem.sync_dir(Path::new("")).unwrap();
    let a = format::encode(0, b"record A");
    file.write_at(&a, 0).unwrap();
    mem.fail_next(Fault::SyncError);
    println!("  write A, sync:         {}", file.sync().unwrap_err());
    println!("  read back:             {}", show(&*file, 0));
    let b = format::encode(a.len() as u64, b"record B");
    file.write_at(&b, a.len() as u64).unwrap();
    file.sync().unwrap();
    println!("  write B, sync:         ok");
    mem.crash(0);
    let file = mem.open(Path::new("log"), Open::Existing).unwrap();
    println!("  after a crash, at 0:   {}", show(&*file, 0));
    println!(
        "  after a crash, at {}:  {}",
        a.len(),
        show(&*file, a.len())
    );

    println!();
    println!("The log, opened again after the failed sync:");
    let mem = Mem::new();
    let vfs = Arc::new(mem.clone());
    let wal = Wal::create(vfs.clone(), "wal", BLOCK as u64).unwrap();
    let lsn = wal.append(b"record A").unwrap();
    mem.fail_next(Fault::SyncError);
    println!(
        "  append A, flush:       {}",
        wal.flush_to(lsn).unwrap_err()
    );
    let wal = Wal::open(vfs.clone(), "wal", BLOCK as u64).unwrap();
    let lsn = wal.append(b"record B").unwrap();
    wal.flush_to(lsn).unwrap();
    println!("  open, append B, flush: ok");
    mem.crash(0);
    let wal = Wal::open(vfs, "wal", BLOCK as u64).unwrap();
    let records: Vec<String> = wal
        .replay()
        .unwrap()
        .map(|r| String::from_utf8(r.unwrap().data).unwrap())
        .collect();
    println!("  after a crash, replay: {records:?}");
}

/// The record in the fragment at `at` in `file`, or what is there instead.
fn show(file: &dyn File, at: usize) -> String {
    let mut block = vec![0; file.len().unwrap() as usize];
    file.read_at(&mut block, 0).unwrap();
    match format::decode(&block, at) {
        Some(f) => format!("{:?}", String::from_utf8_lossy(f.data)),
        None if block[at..at + 7].iter().all(|&b| b == 0) => "zeros, no fragment".into(),
        None => "no fragment".into(),
    }
}
