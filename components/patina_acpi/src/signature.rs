//! ACPI Constants.
//!
//! Defines common constants and table signatures for the ACPI service interface.
//! The following definitions only support ACPI 2.0+.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use core::mem;

use patina::signature;

use crate::acpi_table::{AcpiDsdt, AcpiFacs, AcpiFadt, AcpiTableHeader};

// Helpers for handling ACPI signatures

pub const FACS: u32 = signature!('F', 'A', 'C', 'S');
pub const UEFI: u32 = signature!('U', 'E', 'F', 'I');
pub const FACP: u32 = signature!('F', 'A', 'C', 'P');
pub const DSDT: u32 = signature!('D', 'S', 'D', 'T');
pub const XSDT: u32 = signature!('X', 'S', 'D', 'T');
pub const FADT: u32 = FACP; // For legacy ACPI reasons, the FADT has signature 'FACP'.
pub const MADT: u32 = signature!('M', 'A', 'D', 'T');
pub const HPET: u32 = signature!('H', 'P', 'E', 'T');
pub const MCFG: u32 = signature!('M', 'C', 'F', 'G');
pub const BGRT: u32 = signature!('B', 'G', 'R', 'T');
pub const SRAT: u32 = signature!('S', 'R', 'A', 'T');
pub const SLIT: u32 = signature!('S', 'L', 'I', 'T');
pub const CPEP: u32 = signature!('C', 'P', 'E', 'P');
pub const MSCT: u32 = signature!('M', 'S', 'C', 'T');
pub const RASF: u32 = signature!('R', 'A', 'S', 'F');
pub const RAS2: u32 = signature!('R', 'A', 'S', '2');
pub const MPST: u32 = signature!('M', 'P', 'S', 'T');
pub const PMTT: u32 = signature!('P', 'M', 'T', 'T');
pub const GTDT: u32 = signature!('G', 'T', 'D', 'T');
pub const SBST: u32 = signature!('S', 'B', 'S', 'T');
pub const ECDT: u32 = signature!('E', 'C', 'D', 'T');
pub const NFIT: u32 = signature!('N', 'F', 'I', 'T');
pub const NHLT: u32 = signature!('N', 'H', 'L', 'T');
pub const HMAT: u32 = signature!('H', 'M', 'A', 'T');
pub const PDTT: u32 = signature!('P', 'D', 'T', 'T');
pub const VIOT: u32 = signature!('V', 'I', 'O', 'T');
pub const CCEL: u32 = signature!('C', 'C', 'E', 'L');
pub const SKVL: u32 = signature!('S', 'K', 'V', 'L');
pub const RHCT: u32 = signature!('R', 'H', 'C', 'T');

pub const ACPI_TABLE_GUID: patina::BinaryGuid = patina::BinaryGuid::from_string("8868E871-E4F1-11D3-BC22-0080C73C8881");

pub(crate) const ACPI_HEADER_LEN: usize = mem::size_of::<AcpiTableHeader>();
pub(crate) const MAX_INITIAL_ENTRIES: usize = 32;
pub(crate) const ACPI_CHECKSUM_OFFSET: usize = memoffset::offset_of!(AcpiTableHeader, checksum);

pub const ACPI_RSDP_TABLE: u64 = signature!('R', 'S', 'D', ' ', 'P', 'T', 'R', ' ');
pub const ACPI_RSDP_REVISION: u8 = 2;

pub const ACPI_XSDT_REVISION: u8 = 1;
pub(crate) const ACPI_XSDT_ENTRY_SIZE: usize = core::mem::size_of::<u64>();

pub const ACPI_RESERVED_BYTE: u8 = 0x00;

/// Bitmask indicating versions support of all ACPI versions 2.0+.
pub const ACPI_VERSIONS_GTE_2: u32 = (1 << 2) | (1 << 3) | (1 << 4) | (1 << 5);

// Sizing information for known ACPI table formats.
// Note that for many these are minimum sizes since many tables contain a variable-length portion at the end.
//
// Each minimum size is computed as: (last fixed-field byte offset) + (last fixed-field byte length).
// Variable-length trailing fields (arrays, strings, etc.) are excluded.
pub const FADT_SIZE: usize = mem::size_of::<AcpiFadt>();
pub const FACS_SIZE: usize = mem::size_of::<AcpiFacs>();
pub const DSDT_SIZE: usize = mem::size_of::<AcpiDsdt>();
pub const MADT_SIZE: usize = 44; // Header(36) + Local IC Address(4) + Flags(4)
pub const HPET_SIZE: usize = 56; // Defined by IA-PC HPET spec
pub const MCFG_SIZE: usize = 44; // Defined by PCI Firmware spec
pub const BGRT_SIZE: usize = 56; // Header(36) + Version(2) + Status(1) + ImageType(1) + ImageAddress(8) + ImageOffsetX(4) + ImageOffsetY(4)
pub const GTDT_SIZE: usize = 104; // Header(36) + fixed timer fields through Virtual EL2 Timer Flags(@100, 4)
pub const SBST_SIZE: usize = 48; // Header(36) + Warning(4) + Low(4) + Critical(4)
pub const ECDT_SIZE: usize = 65; // Header(36) + EC_CONTROL GAS(12) + EC_DATA GAS(12) + UID(4) + GPE_BIT(1); EC_ID string follows
pub const SRAT_SIZE: usize = 48; // Header(36) + Reserved(4) + Reserved(8)
pub const SLIT_SIZE: usize = 44; // Header(36) + Number of System Localities(8)
pub const CPEP_SIZE: usize = 44; // Header(36) + Reserved(8)
pub const MSCT_SIZE: usize = 56; // Header(36) + OffsetProxDomInfo(4) + MaxProxDomains(4) + MaxClockDomains(4) + MaxPhysAddr(8)
pub const RASF_SIZE: usize = 48; // Header(36) + PCC Identifier(12)
pub const RAS2_SIZE: usize = 40; // Header(36) + Reserved(2) + Number of PCC Descriptors(2)
pub const MPST_SIZE: usize = 42; // Header(36) + PCC Identifier(1) + Reserved(3) + Memory Power Node Count(2)
pub const PMTT_SIZE: usize = 40; // Header(36) + Number of Memory Devices(4)
pub const NFIT_SIZE: usize = 40; // Header(36) + Reserved(4)
pub const NHLT_SIZE: usize = 37; // Header(36) + Endpoints Count(1)
pub const HMAT_SIZE: usize = 40; // Header(36) + Reserved(4)
pub const PDTT_SIZE: usize = 44; // Header(36) + Trigger Count(1) + Reserved(3) + Trigger ID Array Offset(4)
pub const VIOT_SIZE: usize = 48; // Header(36) + Node Count(2) + Node Offset(2) + Reserved(8)
pub const CCEL_SIZE: usize = 56; // Header(36) + CC Type(1) + CC Subtype(1) + Reserved(2) + LAML(8) + LASA(8)
pub const SKVL_SIZE: usize = 40; // Header(36) + Key Count(4)
pub const RHCT_SIZE: usize = 56; // Header(36) + Flags(4) + Time Base Frequency(8) + Node Count(4) + Node Array Offset(4)

pub fn acpi_table_min_size(signature: u32) -> usize {
    match signature {
        FACS => FACS_SIZE,
        FACP => FADT_SIZE,
        DSDT => DSDT_SIZE,
        MADT => MADT_SIZE,
        HPET => HPET_SIZE,
        MCFG => MCFG_SIZE,
        BGRT => BGRT_SIZE,
        GTDT => GTDT_SIZE,
        SBST => SBST_SIZE,
        ECDT => ECDT_SIZE,
        SRAT => SRAT_SIZE,
        SLIT => SLIT_SIZE,
        CPEP => CPEP_SIZE,
        MSCT => MSCT_SIZE,
        RASF => RASF_SIZE,
        RAS2 => RAS2_SIZE,
        MPST => MPST_SIZE,
        PMTT => PMTT_SIZE,
        NFIT => NFIT_SIZE,
        NHLT => NHLT_SIZE,
        HMAT => HMAT_SIZE,
        PDTT => PDTT_SIZE,
        VIOT => VIOT_SIZE,
        CCEL => CCEL_SIZE,
        SKVL => SKVL_SIZE,
        RHCT => RHCT_SIZE,
        _ => ACPI_HEADER_LEN, // Default to the header size for unknown signatures, or tables without additional size information.
    }
}
