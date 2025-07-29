//! ACPI Constants.
//!
//! Defines common constants and table signatures for the ACPI service interface.
//! The following definitions only support ACPI 2.0+.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent

use r_efi::efi;

use crate::acpi_table::AcpiTableHeader;

// Helpers for handling ACPI signatures

pub const FACS: u32 = 0x53434146;
pub const UEFI: u32 = 0x49464555;
pub const FACP: u32 = 0x50434146;
pub const DSDT: u32 = 0x54445344;
pub const XSDT: u32 = 0x54445358;
pub const FADT: u32 = FACP; // For legacy ACPI reasons, the FADT has signature 'FACP'.

pub const ACPI_TABLE_GUID: efi::Guid =
    efi::Guid::from_fields(0x8868E871, 0xE4F1, 0x11D3, 0xBC, 0x22, &[0x00, 0x80, 0xC7, 0x3C, 0x88, 0x81]);

pub(crate) const ACPI_HEADER_LEN: usize = 36;
pub(crate) const MAX_INITIAL_ENTRIES: usize = 32;
pub(crate) const ACPI_CHECKSUM_OFFSET: usize = memoffset::offset_of!(AcpiTableHeader, checksum);

pub const ACPI_RSDP_TABLE: u64 = 0x2052545020445352;
pub const ACPI_RSDP_REVISION: u8 = 2;

pub const ACPI_XSDT_REVISION: u8 = 1;
pub(crate) const ACPI_XSDT_ENTRY_SIZE: usize = core::mem::size_of::<u64>();

pub const ACPI_RESERVED_BYTE: u8 = 0x00;

/// Bitmask indicating versions support of all ACPI versions 2.0+.
pub const ACPI_VERSIONS_GTE_2: u32 = (1 << 2) | (1 << 3) | (1 << 4) | (1 << 5);
