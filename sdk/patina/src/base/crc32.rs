//! CRC-32 checksum calculation for Patina.
//!
//! Provides a general-purpose CRC-32 checksum function for use across Patina.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

/// Reflected CRC-32 (IEEE 802.3) polynomial.
const POLYNOMIAL: u32 = 0xEDB8_8320;

/// Byte-wise lookup table, generated at compile time.
// Indexing is bounded: the loop index is < 256 and the lookup index is a `u8` into a 256-entry table.
#[allow(clippy::indexing_slicing)]
static TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = i as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ POLYNOMIAL } else { crc >> 1 };
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
};

/// Computes the CRC-32 of `data`, matching what is expected from the UEFI `CalculateCrc32`
/// boot service.
///
/// # Examples
///
/// ```rust,no_run
/// use patina::crc32::calculate_crc32;
///
/// let checksum = calculate_crc32(b"hello");
/// log::info!("crc32 = {checksum:#x}");
/// ```
#[allow(clippy::indexing_slicing)]
pub fn calculate_crc32(data: &[u8]) -> u32 {
    !data.iter().fold(!0u32, |crc, &byte| (crc >> 8) ^ TABLE[((crc as u8) ^ byte) as usize])
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_crc32_empty_slice() {
        assert_eq!(calculate_crc32(&[]), 0);
    }

    #[test]
    fn test_calculate_crc32_known_vector() {
        // "123456789" is the standard CRC-32 check-value test vector.
        let data = b"123456789";
        assert_eq!(calculate_crc32(data), 0xCBF4_3926);
    }

    #[test]
    fn test_calculate_crc32_known_binary_vectors() {
        let data: alloc::vec::Vec<u8> = (0..1024u32).map(|i| (i.wrapping_mul(31) ^ (i >> 3)) as u8).collect();
        for (len, expected) in [
            (1, 0xD202_EF8D),
            (2, 0xCCD1_1F0A),
            (3, 0xF37A_CA27),
            (7, 0xAC70_26DF),
            (8, 0x3BCD_3A9E),
            (15, 0x81F7_67E4),
            (16, 0xF337_EC5F),
            (17, 0x7A96_0734),
            (255, 0xF14F_B093),
            (256, 0xE1F0_BDD8),
            (257, 0x6185_651E),
            (1024, 0xCFB5_99C9),
        ] {
            assert_eq!(calculate_crc32(&data[..len]), expected, "length {len}");
        }
    }

    #[test]
    fn test_calculate_crc32_different_inputs_produce_different_hashes() {
        assert_ne!(calculate_crc32(b"abc"), calculate_crc32(b"abd"));
    }
}
