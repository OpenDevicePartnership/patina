//! CRC-32 checksum calculation for Patina.
//!
//! Provides a general-purpose CRC-32 checksum function for use across Patina.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

/// Computes the CRC-32 of `data`, matching the UEFI `CalculateCrc32` boot service.
///
/// # Examples
///
/// ```rust,no_run
/// use patina::crc32::calculate_crc32;
///
/// let checksum = calculate_crc32(b"hello");
/// log::info!("crc32 = {checksum:#x}");
/// ```
pub fn calculate_crc32(data: &[u8]) -> u32 {
    crc32fast::hash(data)
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_crc32_empty_slice() {
        assert_eq!(calculate_crc32(&[]), crc32fast::hash(&[]));
    }

    #[test]
    fn test_calculate_crc32_known_vector() {
        // "123456789" is the standard CRC-32 check-value test vector.
        let data = b"123456789";
        assert_eq!(calculate_crc32(data), crc32fast::hash(data));
    }

    #[test]
    fn test_calculate_crc32_different_inputs_produce_different_hashes() {
        assert_ne!(calculate_crc32(b"abc"), calculate_crc32(b"abd"));
    }
}
