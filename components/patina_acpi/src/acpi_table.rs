//! ACPI Table Definitions.
//!
//! Defines standard formats for system ACPI tables.
//! Supports only ACPI version >= 2.0.
//! Fields corresponding to ACPI 1.0 are preceded with an underscore (`_`) and are not in use.

use crate::service::TableKey;
use crate::signature;

use core::any::Any;
use core::ptr::addr_of;
use downcast_rs::{impl_downcast, DowncastSync};

/// Represents the FADT for ACPI 2.0+.
/// Equivalent to EFI_ACPI_3_0_FIXED_ACPI_DESCRIPTION_TABLE.
#[repr(C, packed)]
#[derive(Default, Clone, Copy)]
pub struct AcpiFadt {
    // Standard ACPI header.
    pub(crate) header: AcpiTable,

    pub(crate) _firmware_ctrl: u32,
    pub(crate) _dsdt: u32,
    pub(crate) _reserved0: u8,

    pub(crate) preferred_pm_profile: u8,
    pub(crate) sci_int: u16,
    pub(crate) smi_cmd: u32,
    pub(crate) acpi_enable: u8,
    pub(crate) acpi_disable: u8,
    pub(crate) s4bios_req: u8,
    pub(crate) pstate_cnt: u8,
    pub(crate) pm1a_evt_blk: u32,
    pub(crate) pm1b_evt_blk: u32,
    pub(crate) pm1a_cnt_blk: u32,
    pub(crate) pm1b_cnt_blk: u32,
    pub(crate) pm2_cnt_blk: u32,
    pub(crate) pm_tmr_blk: u32,
    pub(crate) gpe0_blk: u32,
    pub(crate) gpe1_blk: u32,
    pub(crate) pm1_evt_len: u8,
    pub(crate) pm1_cnt_len: u8,
    pub(crate) pm2_cnt_len: u8,
    pub(crate) pm_tmr_len: u8,
    pub(crate) gpe0_blk_len: u8,
    pub(crate) gpe1_blk_len: u8,
    pub(crate) gpe1_base: u8,
    pub(crate) cst_cnt: u8,
    pub(crate) p_lvl2_lat: u16,
    pub(crate) p_lvl3_lat: u16,
    pub(crate) flush_size: u16,
    pub(crate) flush_stride: u16,
    pub(crate) duty_offset: u8,
    pub(crate) duty_width: u8,
    pub(crate) day_alrm: u8,
    pub(crate) mon_alrm: u8,
    pub(crate) century: u8,
    pub(crate) ia_pc_boot_arch: u16,
    pub(crate) reserved1: u8,
    pub(crate) flags: u32,
    pub(crate) reset_reg: GenericAddressStructure,
    pub(crate) reset_value: u8,
    pub(crate) reserved2: [u8; 3],

    /// Addresses of the FACS and DSDT (64-bit)
    pub(crate) x_firmware_ctrl: u64,
    pub(crate) x_dsdt: u64,

    pub(crate) x_pm1a_evt_blk: GenericAddressStructure,
    pub(crate) x_pm1b_evt_blk: GenericAddressStructure,
    pub(crate) x_pm1a_cnt_blk: GenericAddressStructure,
    pub(crate) x_pm1b_cnt_blk: GenericAddressStructure,
    pub(crate) x_pm2_cnt_blk: GenericAddressStructure,
    pub(crate) x_pm_tmr_blk: GenericAddressStructure,
    pub(crate) x_gpe0_blk: GenericAddressStructure,
    pub(crate) x_gpe1_blk: GenericAddressStructure,
}

/// Represents an ACPI address space for ACPI 2.0+.
/// Equivalent to EFI_ACPI_3_0_GENERIC_ADDRESS_STRUCTURE.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GenericAddressStructure {
    address_space_id: u8,
    register_bit_width: u8,
    register_bit_offset: u8,
    access_size: u8,
    address: u64,
}

/// Reads unaligned fields on the FADT.
/// Fields on the FADT may be unaligned, since by specification the FADT is packed.
impl AcpiFadt {
    /// SAFETY: reads the packed `x_firmware_ctrl` field even if unaligned.
    #[allow(dead_code)]
    pub unsafe fn get_x_firmware_ctrl(&self) -> u64 {
        // Compute byte offset of packed field
        let p: *const u64 = addr_of!(self.x_firmware_ctrl);

        // Read 8 bytes
        core::ptr::read_unaligned(p)
    }

    /// SAFETY: reads the packed `x_dsdt` field even if unaligned.
    #[allow(dead_code)]
    pub unsafe fn get_x_dsdt(&self) -> u64 {
        // Compute byte offset of packed field
        let p: *const u64 = addr_of!(self.x_dsdt);

        // Read 8 bytes
        core::ptr::read_unaligned(p)
    }
}

/// Represents the FACS for ACPI 2.0+.
/// Note that the FACS does not have a standard ACPI header.
/// The FACS is not present in the list of installed ACPI tables; instead, it is only accessible through the FADT's `x_firmware_ctrl` field.
/// Equivalent to EFI_ACPI_3_0_FIRMWARE_ACPI_CONTROL_STRUCTURE.
#[repr(C, packed)]
#[derive(Default)]
pub struct AcpiFacs {
    pub(crate) signature: u32,
    pub(crate) length: u32,
    pub(crate) hardware_signature: u32,

    pub(crate) _firmware_waking_vector: u32,

    pub(crate) global_lock: u32,
    pub(crate) flags: u32,
    pub(crate) firmware_waking_vector: u64,
    pub(crate) version: u8,
    pub(crate) reserved: [u8; 31],
}

/// Represents the DSDT for ACPI 2.0+.
/// The DSDT is not present in the list of installed ACPI tables; instead, it is only accessible through the FADT's `x_dsdt` field.
/// The DSDT has a standard header followed by variable-length AML bytecode.
/// The `length` field of the header tells us the number of trailing bytes representing bytecode.
#[repr(C, packed)]
#[derive(Default)]
pub struct AcpiDsdt {
    pub(crate) header: AcpiTable,
}

/// Represents the RSDP for ACPI 2.0+.
/// The RSDP is not a standard ACPI table and does not have a standard header.
/// It is not present in the list of installed tables and is not directly accessible.
/// Equivalent to EFI_ACPI_3_0_ROOT_SYSTEM_DESCRIPTION_POINTER.
#[repr(C, packed)]
#[derive(Default)]
pub struct AcpiRsdp {
    pub(crate) signature: u64,

    pub(crate) _checksum: u8,

    pub(crate) oem_id: [u8; 6],
    pub(crate) revision: u8,

    pub(crate) _rsdt_address: u32,

    pub(crate) length: u32,
    pub(crate) xsdt_address: u64,
    pub(crate) extended_checksum: u8,
    pub(crate) reserved: [u8; 3],
}

/// Represents the DSDT for ACPI 2.0+.
/// The DSDT is not present in the list of installed ACPI tables; instead, it is only accessible through the FADT's `x_dsdt` field.
/// The XSDT has a standard header followed by 64-bit addresses of installed tables.
/// The `length` field of the header tells us the number of trailing bytes representing table entries.
#[repr(C)]
#[derive(Default, Copy, Clone)]
pub struct AcpiXsdt {
    pub(crate) header: AcpiTable,
}

/// Represents an installable ACPI table.
/// This is either a standard format ACPI table (`AcpiTable`) or FACS (`AcpiFACS`).
/// Callers of `install_acpi_table` can implement `AcpiInstallable` on custom table formats as well.
pub trait AcpiInstallable: Any + DowncastSync {
    /// The physical address of the table in memory. This is only valid if the table has been installed.
    fn phys_addr(&self) -> Option<usize>;

    fn length(&self) -> u32;
    fn signature(&self) -> u32;
}
// Allows dispatching based on the ACPI table format.
impl_downcast!(sync AcpiInstallable);

/// Represents a standard ACPI header.
/// Equivalent to EFI_ACPI_DESCRIPTION_HEADER.
#[repr(C)]
#[derive(Default, Clone, Debug, Copy)]
pub struct AcpiTable {
    pub signature: u32,
    pub length: u32,
    pub revision: u8,
    pub checksum: u8,
    pub oem_id: [u8; 6],
    pub oem_table_id: [u8; 8],
    pub oem_revision: u32,
    pub creator_id: u32,
    pub creator_revision: u32,
}

/// Wrapper around C-based `AcpiTable` with additional fields for Rust implementation convenience
#[derive(Default, Clone, Debug)]
pub struct AcpiTableWrapper {
    /// Standard ACPI header.
    /// All tables have the standard header except the RSDP and FACS.
    pub header: AcpiTable,

    /* The following fields are not present in the ACPI specification, but are included for implementation convenience. */
    /// Unique key assigned to ACPI table upon installation.
    pub(crate) table_key: TableKey,
    /// Physical address of the table in memory. None if the table is not yet installed in ACPI firmware memory.
    pub(crate) physical_address: Option<usize>,
}

impl AcpiTableWrapper {
    /// Revision number of the table.
    pub fn revision(&self) -> u8 {
        self.header.revision
    }

    /// Header checksum (all bytes in the table must sum to zero).
    pub fn checksum(&self) -> u8 {
        self.header.checksum
    }

    /// OEM ID (6 ASCII characters, not null-terminated).
    pub fn oem_id(&self) -> [u8; 6] {
        self.header.oem_id
    }

    /// OEM Table ID (8 ASCII characters, not null-terminated).
    pub fn oem_table_id(&self) -> [u8; 8] {
        self.header.oem_table_id
    }

    /// OEM revision number.
    pub fn oem_revision(&self) -> u32 {
        self.header.oem_revision
    }

    /// Creator ID (often the compiler/vendor signature).
    pub fn creator_id(&self) -> u32 {
        self.header.creator_id
    }

    /// Creator revision number.
    pub fn creator_revision(&self) -> u32 {
        self.header.creator_revision
    }
}

impl AcpiInstallable for AcpiTableWrapper {
    fn phys_addr(&self) -> Option<usize> {
        self.physical_address
    }

    fn length(&self) -> u32 {
        self.header.length
    }

    fn signature(&self) -> u32 {
        self.header.signature
    }
}

// The FACS has a structure that cannot be coerced into a generic AcpiTable, hence the trait implementation
impl AcpiInstallable for AcpiFacs {
    fn phys_addr(&self) -> Option<usize> {
        Some(self as *const _ as usize)
    }

    fn length(&self) -> u32 {
        self.length
    }

    fn signature(&self) -> u32 {
        signature::FACS
    }
}
