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

use core::mem;

use r_efi::efi;

use crate::acpi_table::AcpiDsdt;
use crate::acpi_table::AcpiFacs;
use crate::acpi_table::AcpiFadt;

// Helpers for handling ACPI signatures

pub const FACS: u32 = 0x53434146;
pub const UEFI: u32 = 0x49464555;
pub const FACP: u32 = 0x50434146;
pub const DSDT: u32 = 0x54445344;
pub const XSDT: u32 = 0x54445358;
pub const FADT: u32 = FACP; // For legacy ACPI reasons, the FADT has signature 'FACP'.
pub const MADT: u32 = 0x5444414D;
pub const HPET: u32 = 0x54455048;
pub const MCFG: u32 = 0x4746434D;
pub const BGRT: u32 = 0x54475242;

pub const ACPI_TABLE_GUID: efi::Guid =
    efi::Guid::from_fields(0x8868E871, 0xE4F1, 0x11D3, 0xBC, 0x22, &[0x00, 0x80, 0xC7, 0x3C, 0x88, 0x81]);

pub const ACPI_HEADER_LEN: usize = 36;

pub const ACPI_RSDP_TABLE: u64 = 0x2052545020445352;
pub const ACPI_RSDP_REVISION: u8 = 2;

pub const ACPI_XSDT_REVISION: u8 = 1;

pub const ACPI_RESERVED_BYTE: u8 = 0x00;

/// Bitmask indicating versions support of all ACPI versions 2.0+.
pub const ACPI_VERSIONS_GTE_2: u32 = (1 << 2) | (1 << 3) | (1 << 4) | (1 << 5);

// Sizing information for known ACPI table formats.
// Note that for many these are minimum sizes since many tables contain a variable-length portion at the end.
pub const FADT_SIZE: usize = mem::size_of::<AcpiFadt>();
pub const FACS_SIZE: usize = mem::size_of::<AcpiFacs>();
pub const DSDT_SIZE: usize = mem::size_of::<AcpiDsdt>();
pub const MADT_SIZE: usize = 44;
pub const HPET_SIZE: usize = 56;
pub const MCFG_SIZE: usize = 44;
pub const BGRT_SIZE: usize = 56;

pub fn acpi_table_min_size(signature: u32) -> usize {
    match signature {
        FACS => FACS_SIZE,
        FACP => FADT_SIZE,
        DSDT => DSDT_SIZE,
        MADT => MADT_SIZE,
        HPET => HPET_SIZE,
        MCFG => MCFG_SIZE,
        BGRT => BGRT_SIZE,
        _ => ACPI_HEADER_LEN, // Default to the header size for unknown signatures, or tables without additional size information.
    }
}

pub const DEFAULT_ACPI_TIMER_FREQUENCY: u64 = 3_579_545; // 3.579545 MHz
