//! Macro definitions for the UEFI SDK.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

/// Converts a size in bytes to the number of UEFI pages required.
///
/// Takes a size in bytes and calculates the number of UEFI pages needed to accommodate that size.
///
/// # Parameters
///
/// - `$size`: The size in bytes that needs to be converted to UEFI pages.
///
/// # Returns
///
/// The number of UEFI pages required to accommodate the given size.
///
/// # Example
///
/// ```rust
/// use patina_sdk::base::UEFI_PAGE_SIZE;
/// use patina_sdk::uefi_size_to_pages;
///
/// let size_in_bytes = UEFI_PAGE_SIZE * 3;
/// let pages = uefi_size_to_pages!(size_in_bytes);
/// assert_eq!(pages, 3);
/// ```
///
/// In this example, 3 UEFI pages are required.
#[macro_export]
macro_rules! uefi_size_to_pages {
    ($size:expr) => {
        (($size) + patina_sdk::base::UEFI_PAGE_MASK) / patina_sdk::base::UEFI_PAGE_SIZE
    };
}

/// Converts a number of UEFI pages to the corresponding size in bytes.
///
/// This macro calculates the total size in bytes by multiplying the given number of UEFI pages
/// by the size of a UEFI page (`UEFI_PAGE_SIZE`).
///
/// # Parameters
///
/// - `$pages`: The number of UEFI pages to be converted to bytes.
///
/// # Returns
///
/// The total size in bytes corresponding to the given number of UEFI pages.
///
/// # Example
///
/// ```rust
/// use patina_sdk::base::UEFI_PAGE_SIZE;
/// use patina_sdk::uefi_pages_to_size;
///
/// let pages = 3;
/// let size_in_bytes = uefi_pages_to_size!(pages);
/// assert_eq!(size_in_bytes, 3 * UEFI_PAGE_SIZE);
/// ```
///
/// In this example, 3 UEFI pages returns the expected size in bytes.
#[macro_export]
macro_rules! uefi_pages_to_size {
    ($pages:expr) => {
        ($pages) * $crate::base::UEFI_PAGE_SIZE
    };
}

/// Macro definitions for working with PCI devices.
pub mod pci {
    /// Constructs a PCI library address from the given bus, device, function, and register values.
    ///
    /// # Arguments
    ///
    /// * `bus` - The PCI bus number (8 bits).
    /// * `device` - The PCI device number (5 bits).
    /// * `function` - The PCI function number (3 bits).
    /// * `register` - The PCI register offset (12 bits).
    ///
    /// # Returns
    ///
    /// A `u32` value representing the PCI library address.
    #[macro_export]
    macro_rules! pci_address {
        ($bus:expr, $device:expr, $function:expr, $register:expr) => {
            (($register & 0xfff) | (($function & 0x07) << 12) | (($device & 0x1f) << 15) | (($bus & 0xff) << 20)) as u32
        };
    }
}
