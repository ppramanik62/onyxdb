//! CRC-32C, the checksum in every fragment header.
//!
//! This is the Castagnoli CRC: polynomial 0x1EDC6F41, processed low bit
//! first, starting from all ones and inverted at the end. It's the CRC that
//! iSCSI, ext4 and InnoDB use, and many CPUs have an instruction for it. This
//! version uses a 256-entry table instead, built when the crate compiles.
//!
//! A CRC is the right tool here because the damage we expect is a few
//! flipped bits or a torn write, and a 32-bit CRC catches every burst of
//! errors up to 32 bits long. It isn't meant to stop someone who changes the
//! file on purpose; for that you'd need a cryptographic hash.
//!
//! Using a table instead of the CPU instruction keeps the crate in plain,
//! safe Rust, with no `unsafe` and nothing that depends on the CPU. The table
//! version is slower, about one table lookup per byte, but a block is read
//! from disk far more slowly than that.

/// The Castagnoli polynomial 0x1EDC6F41 with its bits reversed, because this
/// CRC takes the low bit of each byte first.
const POLY: u32 = 0x82F6_3B78;

/// For each byte value, the CRC remainder of that byte on its own.
const TABLE: [u32; 256] = make_table();

/// Builds `TABLE` by dividing each byte value by the polynomial, one bit at a time.
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
/// use wal::crc::crc32c;
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
