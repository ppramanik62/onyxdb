//! Records cut into fragments and packed into blocks, checked against
//! LevelDB's documented example.

use wal::{
    BLOCK, HEADER,
    format::{Kind, decode, encode},
};

/// Appends a record of `len` bytes to `log`, a log held in memory, and
/// returns its LSN.
fn append(log: &mut Vec<u8>, len: usize) -> u64 {
    log.extend(encode(log.len() as u64, &vec![b'x'; len]));
    log.len() as u64
}

/// Every fragment in `log`: where it starts, its kind and its data length.
fn fragments(log: &[u8]) -> Vec<(usize, Kind, usize)> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < log.len() {
        let block = pos / BLOCK * BLOCK;
        if block + BLOCK - pos < HEADER {
            pos = block + BLOCK;
            continue;
        }
        let bytes = &log[block..log.len().min(block + BLOCK)];
        let fragment = decode(bytes, pos - block).unwrap();
        out.push((pos, fragment.kind, fragment.data.len()));
        pos += HEADER + fragment.data.len();
    }
    out
}

#[test]
fn leveldb_example() {
    // LevelDB's log format doc: A is 1,000 bytes, B is 97,270 and C is 8,000.
    let mut log = Vec::new();
    let a = append(&mut log, 1000);
    let b = append(&mut log, 97270);
    let c = append(&mut log, 8000);
    let expected = [
        (0, Kind::Full, 1000),
        (1007, Kind::First, 31754),
        (32768, Kind::Middle, 32761),
        (65536, Kind::Last, 32755),
        (98304, Kind::Full, 8000),
    ];
    assert_eq!(fragments(&log), expected);
    // B ends 6 bytes before block 3: its trailer.
    assert_eq!((a, b, c), (1007, 98298, 106311));
}

#[test]
fn records_near_the_end_of_a_block() {
    // A 10-byte record when 6, 7 and 8 bytes are left in the block.
    for (left, expected) in [
        (6, vec![(BLOCK, Kind::Full, 10)]),
        (
            7,
            vec![(BLOCK - 7, Kind::First, 0), (BLOCK, Kind::Last, 10)],
        ),
        (8, vec![(BLOCK - 8, Kind::First, 1), (BLOCK, Kind::Last, 9)]),
    ] {
        let mut log = Vec::new();
        append(&mut log, BLOCK - left - HEADER);
        append(&mut log, 10);
        assert_eq!(fragments(&log)[1..], expected, "{left} bytes left");
    }
}

#[test]
fn an_empty_record_is_a_header() {
    let bytes = encode(0, b"");
    assert_eq!(bytes.len(), HEADER);
    assert_eq!(decode(&bytes, 0).unwrap().data, b"");
}

#[test]
fn every_flipped_bit_is_caught() {
    let bytes = encode(0, b"put apple 1");
    for i in 0..bytes.len() {
        for bit in 0..8 {
            let mut damaged = bytes.clone();
            damaged[i] ^= 1 << bit;
            assert_eq!(decode(&damaged, 0), None, "byte {i}, bit {bit}");
        }
    }
}
