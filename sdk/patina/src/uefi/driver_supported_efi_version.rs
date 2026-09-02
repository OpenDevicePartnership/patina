//! Temporary local implementation for the `EFI_DRIVER_SUPPORTED_EFI_VERSION_PROTOCOL` protocol,
//! until it is added to `r-efi`.
//!
//! The Driver Supported EFI Version Protocol provides the version of the UEFI Specification that
//! a driver conforms to. It is optionally installed on a driver's image handle to advertise the
//! highest revision of the UEFI Specification that the driver supports.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

/// The GUID for the `EFI_DRIVER_SUPPORTED_EFI_VERSION_PROTOCOL`.
pub const PROTOCOL_GUID: r_efi::base::Guid =
    r_efi::base::Guid::from_fields(0x5c198761, 0x16a8, 0x4e69, 0x97, 0x2c, &[0x89, 0xd6, 0x79, 0x54, 0xf8, 0x1d]);

/// The `EFI_DRIVER_SUPPORTED_EFI_VERSION_PROTOCOL` structure.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Protocol {
    /// The size, in bytes, of this structure.
    pub length: u32,
    /// The UEFI specification revision that the driver conforms to, in the same format as
    /// `EFI_SYSTEM_TABLE.Hdr.Revision` (major revision in the upper 16 bits, minor in the lower 16 bits).
    pub firmware_version: u32,
}
