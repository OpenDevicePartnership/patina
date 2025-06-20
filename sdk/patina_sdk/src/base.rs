//! UEFI Base Definitions
//!
//! Basic definitions for UEFI development.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent

use num_traits;
use r_efi::efi;

use crate::error::EfiError;

/// EFI memory allocation functions work in units of EFI_PAGEs that are 4KB.
/// This should in no way be confused with the page size of the processor.
/// An EFI_PAGE is just the quanta of memory in EFI.
pub const UEFI_PAGE_SIZE: usize = 0x1000;

/// The mask to apply to an address to get the page offset in UEFI.
pub const UEFI_PAGE_MASK: usize = UEFI_PAGE_SIZE - 1;

/// The shift to apply to an address to get the page frame number in UEFI.
pub const UEFI_PAGE_SHIFT: usize = 12;

/// 1KB, 1024 bytes, 0x400, 2^10
pub const SIZE_1KB: usize = 0x400;

/// 2KB, 2048 bytes, 0x800, 2^11
pub const SIZE_2KB: usize = 0x800;

/// 4KB, 4096 bytes, 0x1000, 2^12
pub const SIZE_4KB: usize = 0x1000;

/// 8KB, 8192 bytes, 0x2000, 2^13
pub const SIZE_8KB: usize = 0x2000;

/// 16KB, 16384 bytes, 0x4000, 2^14
pub const SIZE_16KB: usize = 0x4000;

/// 32KB, 32768 bytes, 0x8000, 2^15
pub const SIZE_32KB: usize = 0x8000;

/// 64KB, 65536 bytes, 0x10000, 2^16
pub const SIZE_64KB: usize = 0x10000;

/// 128KB, 0x20000, 2^17
pub const SIZE_128KB: usize = 0x20000;

/// 256KB, 0x40000, 2^18
pub const SIZE_256KB: usize = 0x40000;

/// 512KB, 0x80000, 2^19
pub const SIZE_512KB: usize = 0x80000;

/// 1MB, 0x100000, 2^20
pub const SIZE_1MB: usize = 0x100000;

/// 2MB, 0x200000, 2^21
pub const SIZE_2MB: usize = 0x200000;

/// 4MB, 0x400000, 2^22
pub const SIZE_4MB: usize = 0x400000;

/// 8MB, 0x800000, 2^23
pub const SIZE_8MB: usize = 0x800000;

/// 16MB, 0x1000000, 2^24
pub const SIZE_16MB: usize = 0x1000000;

/// 32MB, 0x2000000, 2^25
pub const SIZE_32MB: usize = 0x2000000;

/// 64MB, 0x4000000, 2^26
pub const SIZE_64MB: usize = 0x4000000;

/// 128MB, 0x8000000, 2^27
pub const SIZE_128MB: usize = 0x8000000;

/// 256MB, 0x10000000, 2^28
pub const SIZE_256MB: usize = 0x10000000;

/// 512MB, 0x20000000, 2^29
pub const SIZE_512MB: usize = 0x20000000;

/// 1GB, 0x40000000, 2^30
pub const SIZE_1GB: usize = 0x40000000;

/// 2GB, 0x80000000, 2^31
pub const SIZE_2GB: usize = 0x80000000;

/// 4GB, 0x100000000, 2^32
pub const SIZE_4GB: usize = 0x100000000;

/// 8GB, 0x200000000, 2^33
pub const SIZE_8GB: usize = 0x200000000;

/// 16GB, 0x400000000, 2^34
pub const SIZE_16GB: usize = 0x400000000;

/// 32GB, 0x800000000, 2^35
pub const SIZE_32GB: usize = 0x800000000;

/// 64GB, 0x1000000000, 2^36
pub const SIZE_64GB: usize = 0x1000000000;

/// 128GB, 0x2000000000, 2^37
pub const SIZE_128GB: usize = 0x2000000000;

/// 256GB, 0x4000000000, 2^38
pub const SIZE_256GB: usize = 0x4000000000;

/// 512GB, 0x8000000000, 2^39
pub const SIZE_512GB: usize = 0x8000000000;

/// 1TB, 0x10000000000, 2^40
pub const SIZE_1TB: usize = 0x10000000000;

/// 2TB, 0x20000000000, 2^41
pub const SIZE_2TB: usize = 0x20000000000;

/// 4TB, 0x40000000000, 2^42
pub const SIZE_4TB: usize = 0x40000000000;

/// 8TB, 0x80000000000, 2^43
pub const SIZE_8TB: usize = 0x80000000000;

/// 16TB, 0x100000000000, 2^44
pub const SIZE_16TB: usize = 0x100000000000;

/// 32TB, 0x200000000000, 2^45
pub const SIZE_32TB: usize = 0x200000000000;

/// 64TB, 0x400000000000, 2^46
pub const SIZE_64TB: usize = 0x400000000000;

/// 128TB, 0x800000000000, 2^47
pub const SIZE_128TB: usize = 0x800000000000;

/// 256TB, 0x1000000000000, 2^48
pub const SIZE_256TB: usize = 0x1000000000000;

/// Patina uses write back as the default cache attribute for memory allocations.
pub const DEFAULT_CACHE_ATTR: u64 = efi::MEMORY_WB;

/// Checks if the given value is a power of two.
/// This function checks if the value `x` is greater than zero and if it is a power of two.
/// # Parameters
/// - `x`: The value to check.
/// # Returns
/// - `true`: If `x` is a power of two.
/// - `false`: If `x` is not a power of two.
#[inline]
pub fn is_power_of_two<T>(x: T) -> bool
where
    T: num_traits::PrimInt,
{
    x > T::zero() && (x & (x - T::one())) == T::zero()
}

/// Aligns the given address down to the nearest boundary specified by align.
///
/// # Parameters
///
/// - `addr`: The address to be aligned.
/// - `align`: The alignment boundary, which must be a power of two.
///
/// # Returns
///
/// A `Result<T, EfiError>` which is:
/// - `Ok(T)`: The aligned address if `align` is a power of two.
/// - `Err(EfiError)`: An error indicating that `align` must be a power of two.
///
/// # Example
///
/// ```rust
/// use patina_sdk::base::align_down;
///
/// let addr: u64 = 1023;
/// let align: u64 = 512;
/// match align_down(addr, align) {
///     Ok(aligned_addr) => {
///         println!("Aligned address: {}", aligned_addr);
///         assert_eq!(aligned_addr, 512);
///     },
///     Err(e) => println!("Error: {:?}", e),
/// }
/// ```
///
/// In this example, the address `1023` is aligned down to `512`.
///
/// # Errors
///
/// The function returns an error if:
/// - `align` is not a power of two.
#[inline]
pub fn align_down<T>(addr: T, align: T) -> Result<T, EfiError>
where
    T: num_traits::PrimInt,
{
    if !is_power_of_two(align) {
        return Err(EfiError::InvalidParameter);
    }
    Ok(addr & !(align - T::one()))
}

/// Aligns the given address up to the nearest boundary specified by align.
///
/// # Parameters
///
/// - `addr`: The address to be aligned.
/// - `align`: The alignment boundary, which must be a power of two.
///
/// # Returns
///
/// A `Result<T, EfiError>` which is:
/// - `Ok(T)`: The aligned address if `align` is a power of two and no overflow occurs.
/// - `Err(EfiError)`: An error indicating the reason for failure (either invalid `align` or overflow).
///
/// # Example
///
/// ```rust
/// use patina_sdk::base::align_up;
/// use patina_sdk::error::EfiError;
///
/// let addr: u64 = 1025;
/// let align: u64 = 512;
/// match align_up(addr, align) {
///     Ok(aligned_addr) => {
///         println!("Aligned address: {}", aligned_addr);
///         assert_eq!(aligned_addr, 1536);
///     },
///     Err(EfiError::InvalidParameter) => println!("Invalid alignment parameter"),
///     Err(_) => println!("Other alignment error"),
/// }
/// ```
///
/// In this example, the address `1025` is aligned up to `1536`.
///
/// # Errors
///
/// The function returns an error if:
/// - `align` is not a power of two.
/// - An overflow occurs during the alignment process.
#[inline]
pub fn align_up<T>(addr: T, align: T) -> Result<T, EfiError>
where
    T: num_traits::PrimInt,
{
    if !is_power_of_two(align) {
        return Err(EfiError::InvalidParameter);
    }
    let align_mask = align - T::one();
    if addr & align_mask == T::zero() {
        Ok(addr) // already aligned
    } else {
        (addr | align_mask).checked_add(&T::one()).ok_or(EfiError::InvalidParameter)
    }
}

/// Aligns the given address down to the nearest boundary specified by align.
/// Also calculates the aligned length based on the base and length provided.
///
/// # Parameters
/// - `base`: The base address to be aligned.
/// - `length`: The length to be aligned.
/// - `align`: The alignment boundary, which must be a power of two.
///
/// # Returns
/// A `Result<(T, T), EfiError>` which is:
/// - `Ok((T, T))`: A tuple containing the aligned base address and the aligned length.
/// - `Err(EfiError)`: An error indicating that `align` must be a power of two.
///
/// # Example
/// ```rust
/// use patina_sdk::base::align_range;
/// let base: u64 = 1023;
/// let length: u64 = 2048;
/// let align: u64 = 512;
/// match align_range(base, length, align) {
///     Ok((aligned_base, aligned_length)) => {
///         println!("Aligned base: {}, Aligned length: {}", aligned_base, aligned_length);
///         assert_eq!(aligned_base, 512);
///         assert_eq!(aligned_length, 2560);
///     },
///     Err(e) => println!("Error: {:?}", e),
/// }
/// ```
///
/// In this example, the base address `1023` is aligned down to `512`, and the length is adjusted accordingly.
/// # Errors
/// The function returns an error if:
/// - `align` is not a power of two.
#[inline]
pub fn align_range<T>(base: T, length: T, align: T) -> Result<(T, T), EfiError>
where
    T: num_traits::PrimInt,
{
    if !is_power_of_two(align) {
        return Err(EfiError::InvalidParameter);
    }

    let aligned_end = align_up(base + length, align)?;
    let aligned_base = align_down(base, align)?;
    let aligned_length = aligned_end - aligned_base;
    Ok((aligned_base, aligned_length))
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_power_of_two() {
        assert!(is_power_of_two(1u64));
        assert!(is_power_of_two(2u64));
        assert!(is_power_of_two(4u64));
        assert!(is_power_of_two(1024u64));
        assert!(!is_power_of_two(0u64));
        assert!(!is_power_of_two(3u64));
        assert!(!is_power_of_two(1023u64));
    }

    #[test]
    fn test_align_down() {
        assert_eq!(align_down(1023u64, 512u64).unwrap(), 512u64);
        assert_eq!(align_down(1024u64, 512u64).unwrap(), 1024u64);
        assert_eq!(align_down(0u64, 512u64).unwrap(), 0u64);
        assert_eq!(align_down(513u64, 512u64).unwrap(), 512u64);
        assert_eq!(align_down(0xFFFFu64, 0x1000u64).unwrap(), 0xF000u64);
        assert_eq!(align_down(0x1000u64, 0x1000u64).unwrap(), 0x1000u64);
        assert!(align_down(100u64, 3u64).is_err()); // not power of two
    }

    #[test]
    fn test_align_up() {
        assert_eq!(align_up(1025u64, 512u64).unwrap(), 1536u64);
        assert_eq!(align_up(1024u64, 512u64).unwrap(), 1024u64);
        assert_eq!(align_up(0u64, 512u64).unwrap(), 0u64);
        assert_eq!(align_up(513u64, 512u64).unwrap(), 1024u64);
        assert_eq!(align_up(0xFFFFu64, 0x1000u64).unwrap(), 0x10000u64);
        assert_eq!(align_up(0x1000u64, 0x1000u64).unwrap(), 0x1000u64);
        assert!(align_up(100u64, 3u64).is_err()); // not power of two
                                                  // Check for overflow
        assert!(align_up(u64::MAX, 2u64).is_err());
    }

    #[test]
    fn test_align_range() {
        let (base, len) = align_range(1023u64, 2048u64, 512u64).unwrap();
        assert_eq!(base, 512u64);
        assert_eq!(len, 2560u64);

        let (base, len) = align_range(0u64, 100u64, 64u64).unwrap();
        assert_eq!(base, 0u64);
        assert_eq!(len, 128u64);

        let (base, len) = align_range(100u64, 100u64, 64u64).unwrap();
        assert_eq!(base, 64u64);
        assert_eq!(len, 192u64);

        assert!(align_range(100u64, 100u64, 3u64).is_err()); // not power of two
    }
}
