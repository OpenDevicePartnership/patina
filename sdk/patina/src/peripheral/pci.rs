//! Definitions for PCI devices and related structures and constants.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

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
