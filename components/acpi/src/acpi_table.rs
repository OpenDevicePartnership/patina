//! ACPI Table Definitions.
//!
//! Defines standard formats for system ACPI tables.
//! Supports only ACPI version >= 2.0.
//! Fields corresponding to ACPI 1.0 are preceded with an underscore (`_`) and are not in use.

use crate::alloc::vec::Vec;

use crate::signature;
use crate::{error::AcpiError, service::TableKey};

use core::any::Any;
use core::ptr::addr_of;
use downcast_rs::{impl_downcast, DowncastSync};

#[repr(C, packed)]
#[derive(Default, Clone, Copy)]
pub struct AcpiFadt {
    // Standard ACPI header.
    pub(crate) signature: u32,
    pub(crate) length: u32,
    pub(crate) revision: u8,
    pub(crate) checksum: u8,
    pub(crate) oem_id: [u8; 6],
    pub(crate) oem_table_id: [u8; 8],
    pub(crate) oem_revision: u32,
    pub(crate) creator_id: u32,
    pub(crate) creator_revision: u32,

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

#[repr(C, packed)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GenericAddressStructure {
    address_space_id: u8,
    register_bit_width: u8,
    register_bit_offset: u8,
    access_size: u8,
    address: u64,
}

impl AcpiFadt {
    /// SAFETY: reads the packed `x_firmware_ctrl` field even if unaligned.
    pub unsafe fn get_x_firmware_ctrl(&self) -> u64 {
        // Compute byte offset of packed field
        let p: *const u64 = addr_of!(self.x_firmware_ctrl);

        // Read 8 bytes
        core::ptr::read_unaligned(p)
    }

    /// SAFETY: reads the packed `x_dsdt` field even if unaligned.
    pub unsafe fn get_x_dsdt(&self) -> u64 {
        // Compute byte offset of packed field
        let p: *const u64 = addr_of!(self.x_dsdt);

        // Read 8 bytes
        core::ptr::read_unaligned(p)
    }
}

impl TryFrom<AcpiTable> for AcpiFadt {
    type Error = AcpiError;

    fn try_from(table: AcpiTable) -> Result<Self, Self::Error> {
        // Verify signature is FACP
        if table.signature != u32::from_le_bytes(*b"FACP") {
            return Err(AcpiError::InvalidSignature);
        }

        // Ensure we have enough data to read the relevant FADT fields
        const XDS_OFFSET: usize = memoffset::offset_of!(AcpiFadt, x_dsdt);
        if table.data.len() < XDS_OFFSET + 8 {
            return Err(AcpiError::InvalidTableLength);
        }

        // Determine total length from header
        let total_len = table.length as usize;

        // Build a raw byte buffer: header fields followed by table.data
        let mut raw = Vec::with_capacity(total_len);
        raw.extend_from_slice(&table.signature.to_le_bytes());
        raw.extend_from_slice(&table.length.to_le_bytes());
        raw.push(table.revision);
        raw.push(table.checksum);
        raw.extend_from_slice(&table.oem_id);
        raw.extend_from_slice(&table.oem_table_id);
        raw.extend_from_slice(&table.oem_revision.to_le_bytes());
        raw.extend_from_slice(&table.creator_id.to_le_bytes());
        raw.extend_from_slice(&table.creator_revision.to_le_bytes());
        // Append the rest of the table body
        raw.extend_from_slice(&table.data);
        // Ensure raw matches expected length
        if raw.len() != total_len {
            return Err(AcpiError::InvalidTableLength);
        }

        // Copy into FADT struct
        let mut fadt = AcpiFadt::default();
        let dest_ptr = &mut fadt as *mut AcpiFadt as *mut u8;
        // SAFETY: dest_ptr has the right size and is valid (allocated above)
        // `raw` gets its data directly from the passed in AcpiTable, so as long as the caller passes in a valid AcpiTable, this is safe
        // and `raw` points to a valid Vec on the Rust heap, which we have just allocated
        unsafe {
            core::ptr::copy_nonoverlapping(raw.as_ptr(), dest_ptr, total_len);
        }
        Ok(fadt)
    }
}

// The FACS does not have a standard ACPI header.
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

#[repr(C, packed)]
#[derive(Default)]
pub struct AcpiDsdt {
    pub(crate) signature: u32,
    pub(crate) length: u32,
    pub(crate) revision: u8,
    pub(crate) checksum: u8,
    pub(crate) oem_id: [u8; 6],
    pub(crate) oem_table_id: [u8; 8],
    pub(crate) oem_revision: u32,
    pub(crate) creator_id: u32,
    pub(crate) creator_revision: u32,
}

// The RSDP is not a standard ACPI table and does not have a standard header.
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

// The XSDT has a standard header followed by 64-bit addresses of installed tables.
// The `length` field tells us the number of trailing bytes representing table entries.
#[repr(C)]
#[derive(Default, Copy, Clone)]
pub struct AcpiXsdt {
    pub(crate) signature: u32,
    pub(crate) length: u32,
    pub(crate) revision: u8,
    pub(crate) checksum: u8,
    pub(crate) oem_id: [u8; 6],
    pub(crate) oem_table_id: [u8; 8],
    pub(crate) oem_revision: u32,
    pub(crate) creator_id: u32,
    pub(crate) creator_revision: u32,
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

/// Represents a standard ACPI table format: a header followed by trailing data determined by `length`.
#[repr(C)]
#[derive(Default, Clone, Debug)]
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
    // Trailing variable-length data that differs between ACPI table types
    pub data: Vec<u8>,
    // Additional fields not present in the original C struct. Included for Rust conveinence
    pub table_key: TableKey, // Unique key assigned to ACPI table upon installation
    pub(crate) physical_address: Option<usize>, // Physical address of the table in memory. None if the table is not yet installed in ACPI firmware memory
}

impl AcpiInstallable for AcpiTable {
    fn phys_addr(&self) -> Option<usize> {
        self.physical_address
    }

    fn length(&self) -> u32 {
        self.length
    }

    fn signature(&self) -> u32 {
        self.signature
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

#[cfg(test)]
mod tests {
    use crate::signature::ACPI_HEADER_LEN;

    use super::*;
    use alloc::vec;
    use core::mem;

    #[test]
    fn try_from_succeeds_with_valid_fadt() {
        // Header bytes
        let sig = u32::from_le_bytes(*b"FACP");
        let rev = 2;
        let chksum = 0xAB;
        let oem_id = *b"OEMID_";
        let oem_table_id = *b"TBLID123";
        let oem_rev = 0xCAFEBABE;
        let creator_id = 0x1234_5678;
        let creator_rev = 0xDEAD_BEEF;

        // Size of rest of FADT (excluding header)
        let body_len = mem::size_of::<AcpiFadt>() - ACPI_HEADER_LEN;

        // This corresponds to the table.data field of AcpiTable
        // The contents are not important
        let payload = vec![0xFE; body_len];

        let table = AcpiTable {
            signature: sig,
            length: mem::size_of::<AcpiFadt>() as u32,
            revision: rev,
            checksum: chksum,
            oem_id,
            oem_table_id,
            oem_revision: oem_rev,
            creator_id,
            creator_revision: creator_rev,
            data: payload.clone(),
            table_key: TableKey::default(),
            physical_address: Some(0),
        };

        let fadt = AcpiFadt::try_from(table).expect("Valid FADT table should parse");

        // Check some header fields
        assert_eq!(fadt.revision, rev);
        assert_eq!(fadt.checksum, chksum);
        assert_eq!(fadt.oem_id, oem_id);
        assert_eq!(fadt.oem_table_id, oem_table_id);

        // Verify the rest of the FADT fields are 0xFE (dummy byte)
        const XDS_OFFSET: usize = memoffset::offset_of!(AcpiFadt, x_dsdt);
        let raw_bytes =
            unsafe { core::slice::from_raw_parts((&fadt as *const AcpiFadt as *const u8).add(XDS_OFFSET), 8) };
        // Should equal the first 8 bytes of our payload (0xFE)
        assert_eq!(raw_bytes, &[0xFE; 8]);
    }

    #[test]
    fn try_from_fails_with_bad_signature() {
        // Build a table with a wrong signature
        let bad_signature_table = AcpiTable {
            signature: 0xDEAD_BEEF,
            length: mem::size_of::<AcpiFadt>() as u32,
            revision: 0,
            checksum: 0,
            oem_id: [0; 6],
            oem_table_id: [0; 8],
            oem_revision: 0,
            creator_id: 0,
            creator_revision: 0,
            data: vec![0; mem::size_of::<AcpiFadt>() - ACPI_HEADER_LEN],
            table_key: TableKey::default(),
            physical_address: Some(0),
        };

        match AcpiFadt::try_from(bad_signature_table) {
            Err(AcpiError::InvalidSignature) => (),
            _ => panic!("Expected InvalidSignature error"),
        }
    }
}
