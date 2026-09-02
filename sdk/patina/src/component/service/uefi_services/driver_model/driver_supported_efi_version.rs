//! Driver Supported EFI Version protocol production for Patina components.
//!
//! A component can install EFI Driver Supported EFI Version protocol with
//! [`install_uefi_driver_model_driver_supported_efi_version`](crate::component::service::uefi_services::protocol::ProtocolServicesExt::install_uefi_driver_model_driver_supported_efi_version)
//! to advertise the highest UEFI Specification revision the component conforms to.
//!
//! Required on drivers that are on PCI and other plug in cards.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use alloc::boxed::Box;
use core::mem::size_of;

use crate::base::guid::BinaryGuid;
use crate::base::protocol::ProtocolInterface;

pub use crate::uefi::driver_supported_efi_version::Protocol;

// SAFETY: `Protocol` is `#[repr(C)]` and matches the layout of
// `EFI_DRIVER_SUPPORTED_EFI_VERSION_PROTOCOL`.
unsafe impl ProtocolInterface for Protocol {
    const PROTOCOL_GUID: BinaryGuid = BinaryGuid(crate::uefi::driver_supported_efi_version::PROTOCOL_GUID);
}

/// Builds an EFI Driver Supported EFI Version protocol instance for `firmware_version`.
///
/// `length` is always `size_of::<Protocol>()`, so callers only need to supply `firmware_version`.
pub(in crate::component::service::uefi_services) fn build_protocol(firmware_version: u32) -> Box<Protocol> {
    Box::new(Protocol { length: size_of::<Protocol>() as u32, firmware_version })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_protocol_sets_length_and_firmware_version() {
        let protocol = build_protocol(0x0002_000B);
        assert_eq!(protocol.length, size_of::<Protocol>() as u32);
        assert_eq!(protocol.firmware_version, 0x0002_000B);
    }

    #[test]
    fn test_protocol_guid_matches_raw_definition() {
        assert_eq!(Protocol::PROTOCOL_GUID, BinaryGuid(crate::uefi::driver_supported_efi_version::PROTOCOL_GUID));
    }
}
