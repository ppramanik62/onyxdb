//! Batches: the limits, the encoding, and decoding bytes that are not a
//! batch.

use std::ops::Bound::{Excluded, Included, Unbounded};

use kv::{Batch, Error, MAX_KEY, MAX_VALUE, empty_range};
use vfs::Rng;

/// A random batch of up to 8 ops, with keys of any length up to the limit,
/// and values of 0, 1, 100 or 5,000 bytes.
fn random_batch(rng: &mut Rng) -> Batch {
    let mut batch = Batch::new();
    for _ in 0..rng.below(9) {
        let len = rng.below(MAX_KEY + 1);
        let key = rng.bytes(len);
        if rng.below(3) == 0 {
            batch.delete(key).unwrap();
        } else {
            let len = [0, 1, 100, 5_000][rng.below(4)];
            let value = rng.bytes(len);
            batch.put(key, value).unwrap();
        }
    }
    batch
}

#[test]
fn limits_are_exact() {
    let mut batch = Batch::new();
    batch.put(vec![1; MAX_KEY], vec![2; MAX_VALUE]).unwrap();
    batch.delete(vec![1; MAX_KEY]).unwrap();
    let key = batch.put(vec![1; MAX_KEY + 1], "").unwrap_err();
    assert!(matches!(key, Error::KeyTooLong { len: 513, max: 512 }));
    let value = batch.put("k", vec![2; MAX_VALUE + 1]).unwrap_err();
    assert!(matches!(
        value,
        Error::ValueTooLong {
            len: 1_048_577,
            max: 1_048_576
        }
    ));
    assert!(batch.delete(vec![1; MAX_KEY + 1]).is_err());
    // The refused ops are not in the batch.
    assert_eq!(batch.len(), 2);
}

#[test]
fn empty_ranges() {
    assert!(empty_range(Included(b"b"), Excluded(b"a")));
    assert!(empty_range(Excluded(b"a"), Excluded(b"a")));
    assert!(empty_range(Excluded(b"a"), Included(b"a")));
    assert!(!empty_range(Included(b""), Included(b"")));
    assert!(!empty_range(Included(b"a"), Unbounded));
    // "a" is a prefix of "a\0", so it comes first.
    assert!(!empty_range(Excluded(b"a"), Included(b"a\0")));
}

#[test]
fn round_trip() {
    let mut rng = Rng::new(1);
    for _ in 0..200 {
        let batch = random_batch(&mut rng);
        assert_eq!(Batch::decode(&batch.encode()).unwrap(), batch);
    }
}

#[test]
fn decode_refuses_bad_bytes() {
    let cases: [(&[u8], &str); 4] = [
        (b"\x03\x00\x00", "has kind 3"),
        (b"\x01\x02\x00k", "2 bytes needed at byte 3"),
        (
            b"\x01\x01\x00k\x05\x00\x00\x00v",
            "5 bytes needed at byte 8",
        ),
        (b"\x02\x01\x02", "513 bytes needed at byte 3"),
    ];
    for (bytes, expected) in cases {
        let err = Batch::decode(bytes).unwrap_err();
        assert!(err.to_string().contains(expected), "{err}");
    }
    // A key length over the limit, with all its bytes present.
    let mut bytes = vec![2, 0x01, 0x02];
    bytes.extend(vec![b'k'; 513]);
    let err = Batch::decode(&bytes).unwrap_err();
    assert_eq!(
        err.to_string(),
        "corrupt: batch: op at byte 0: the key is 513 bytes long, and the limit is 512"
    );
}

#[test]
fn fuzz_decode() {
    let mut rng = Rng::new(2);
    let mut decoded = 0;
    for _ in 0..10_000 {
        let len = rng.below(64);
        let mut bytes = if rng.below(2) == 0 {
            rng.bytes(len)
        } else {
            random_batch(&mut rng).encode()
        };
        // Change one byte, or cut the bytes short.
        if !bytes.is_empty() {
            let i = rng.below(bytes.len());
            match rng.below(2) {
                0 => bytes[i] = rng.next_u64() as u8,
                _ => bytes.truncate(i),
            }
        }
        // Decoding returns a batch or an error, and never panics. A batch
        // that comes back encodes to the same bytes.
        if let Ok(batch) = Batch::decode(&bytes) {
            assert_eq!(batch.encode(), bytes);
            decoded += 1;
        }
    }
    println!("{decoded} of 10000 damaged inputs still decoded");
}
