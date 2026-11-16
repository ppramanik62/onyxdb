//! Shows how pages land in a small page table, and what removing one does.
//!
//!     cargo run --example table

use bufpool::PageTable;
use pagefile::PageNo;

/// Prints every slot: the page in it, its home slot and its frame.
fn show(table: &PageTable<usize>) {
    for i in 0..table.slots() {
        match table.slot(i) {
            Some((page, frame)) => println!(
                "  slot {i}: page {page} (home {}) -> frame {frame}",
                table.home(page)
            ),
            None => println!("  slot {i}: empty"),
        }
    }
}

fn main() {
    let no = |n| PageNo::new(n).unwrap();
    let mut table = PageTable::new(4);
    println!("a table for 4 entries has {} slots", table.slots());
    let homes: Vec<String> = (1..=9)
        .map(|n| format!("{n}->{}", table.home(no(n))))
        .collect();
    println!("home slots of pages 1 to 9: {}", homes.join(" "));

    for (frame, n) in [1, 6, 9].into_iter().enumerate() {
        table.insert(no(n), frame);
    }
    println!("after inserting pages 1, 6 and 9:");
    show(&table);

    table.remove(no(1));
    println!("after removing page 1:");
    show(&table);
    println!("page 9 is still found: frame {:?}", table.get(no(9)));
}
