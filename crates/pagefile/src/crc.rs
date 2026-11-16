//! CRC-32C, the checksum at the end of every page.
//!
//! This is the Castagnoli CRC: the polynomial 0x1EDC6F41, with the low bit
//! of each byte processed first, a start value of all ones, and the result
//! inverted at the end. iSCSI, ext4 and InnoDB use the same CRC. Many CPUs
//! have an instruction for it, but this version uses a table of 256 entries,
//! which is built when the crate compiles.
//!
//! A CRC fits the damage a disk causes: a few flipped bits, or a write that
//! was torn by a crash. A 32-bit CRC detects every group of wrong bits that
//! is 32 bits long or shorter. It does not protect against a person who
//! changes the file on purpose. That needs a cryptographic hash.
//!
//! The table keeps the crate in plain, safe Rust: no `unsafe` code, and
//! nothing that depends on the CPU. The table is slower than the CPU
//! instruction, about one table lookup per byte. That does not matter here,
//! because reading a page from disk takes much longer than checking it.

/// The Castagnoli polynomial 0x1EDC6F41 with its bits reversed, because this
/// CRC takes the low bit of each byte first.
const POLY: u32 = 0x82F6_3B78;

/// For each of the 256 byte values, the CRC remainder of that one byte.
const TABLE: [u32; 256] = make_table();

/// Builds `TABLE` by dividing each byte value by the polynomial, one bit at a
/// time.
const fn make_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = i as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ POLY
            } else {
                crc >> 1
            };
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

/// The CRC-32C of `data`. It reads each byte once, with one table lookup.
///
/// # Examples
///
/// ```
/// use pagefile::crc::crc32c;
///
/// // The published check value: the CRC of the ASCII digits 1 to 9.
/// assert_eq!(crc32c(b"123456789"), 0xE306_9283);
/// assert_eq!(crc32c(b""), 0);
/// ```
pub fn crc32c(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in data {
        crc = TABLE[((crc ^ byte as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}

#[test]
fn check_value() {
    // Every CRC has a published "check" value: the CRC of the nine ASCII
    // digits 1 to 9. This is CRC-32C's.
    assert_eq!(crc32c(b"123456789"), 0xE306_9283);
}

#[test]
fn one_bit_changes_the_crc() {
    let mut data = *b"apple banana cherry";
    let before = crc32c(&data);
    data[3] ^= 0b0000_0100;
    assert_ne!(crc32c(&data), before);
}
