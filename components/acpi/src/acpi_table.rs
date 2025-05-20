use crate::acpi::AcpiVersion;
use crate::signature;
use crate::{error::AcpiError, service::TableKey};

use core::any::Any;
use downcast_rs::{impl_downcast, DowncastSync};

#[repr(C, packed)]
#[derive(Default, Clone, Copy)]
pub struct AcpiFadt {
    pub signature: u32,
    pub(crate) length: u32,
    pub revision: u8,
    pub checksum: u8,
    pub oem_id: [u8; 6],
    pub oem_table_id: [u8; 8],
    pub oem_revision: u32,
    pub creator_id: u32,
    pub creator_revision: u32,

    _firmware_ctrl: u32,
    _dsdt: u32,
    _reserved0: u8,
    _preferred_pm_profile: u8,
    _sci_int: u16,
    _smi_cmd: u32,
    _acpi_enable: u8,
    _acpi_disable: u8,
    _s4bios_req: u8,
    _pstate_cnt: u8,
    _pm1a_evt_blk: u32,
    _pm1b_evt_blk: u32,
    _pm1a_cnt_blk: u32,
    _pm1b_cnt_blk: u32,
    _pm2_cnt_blk: u32,
    _pm_tmr_blk: u32,
    _gpe0_blk: u32,
    _gpe1_blk: u32,
    _pm1_evt_len: u8,
    _pm1_cnt_len: u8,
    _pm2_cnt_len: u8,
    _pm_tmr_len: u8,
    _gpe0_blk_len: u8,
    _gpe1_blk_len: u8,
    _gpe1_base: u8,
    _cst_cnt: u8,
    _p_lvl2_lat: u16,
    _p_lvl3_lat: u16,
    _flush_size: u16,
    _flush_stride: u16,
    _duty_offset: u8,
    _duty_width: u8,
    _day_alrm: u8,
    _mon_alrm: u8,
    _century: u8,
    _ia_pc_boot_arch: u16,
    _reserved1: u8,
    _flags: u32,
    pub reset_reg: GenericAddressStructure,
    pub reset_value: u8,
    _reserved2: [u8; 3],
    pub(crate) x_firmware_ctrl: u64,
    pub(crate) x_dsdt: u64,
    pub x_pm1a_evt_blk: GenericAddressStructure,
    pub x_pm1b_evt_blk: GenericAddressStructure,
    pub x_pm1a_cnt_blk: GenericAddressStructure,
    pub x_pm1b_cnt_blk: GenericAddressStructure,
    pub x_pm2_cnt_blk: GenericAddressStructure,
    pub x_pm_tmr_blk: GenericAddressStructure,
    pub x_gpe0_blk: GenericAddressStructure,
    pub x_gpe1_blk: GenericAddressStructure,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GenericAddressStructure {
    _address_space_id: u8,
    _register_bit_width: u8,
    _register_bit_offset: u8,
    _access_size: u8,
    _address: u64,
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
        unsafe {
            core::ptr::copy_nonoverlapping(raw.as_ptr(), dest_ptr, total_len);
        }
        Ok(fadt)
    }
}

#[repr(C, packed)]
pub struct AcpiFacs {
    pub signature: u32,
    pub length: u32,
    pub hardware_signature: u32,
    _firmware_waking_vector: u32,
    pub global_lock: u32,
    pub flags: u32,
    pub firmware_waking_vector: u64,
    pub version: u8,
    pub reserved: [u8; 31],
}

#[repr(C, packed)]
pub struct AcpiDsdt {
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

#[repr(C, packed)]
pub struct AcpiRsdp {
    pub signature: u64,
    pub(crate) checksum: u8,
    pub oem_id: [u8; 6],
    pub(crate) revision: u8,
    _rsdt_address: u32,
    pub(crate) length: u32,
    pub xsdt_address: u64,
    pub(crate) extended_checksum: u8,
    pub(crate) reserved: [u8; 3],
}

#[repr(C)]
pub struct AcpiXsdt {
    pub signature: u32,
    pub length: u32,
    pub revision: u8,
    pub checksum: u8,
    pub oem_id: [u8; 6],
    pub oem_table_id: [u8; 8],
    pub oem_revision: u32,
    pub creator_id: u32,
    pub creator_revision: u32,

    // For Rust implentation conveinence: current XSDT entries
    pub entries: Vec<u64>,
    // For Rust implentation conveinence: max entries the currently allocated XSDT can hold
    pub(crate) max_entries: usize,
}

pub trait AcpiInstallable: Any + DowncastSync {
    /// The physical address of the table in memory. This is only valid if the table has been installed
    fn phys_addr(&self) -> Option<usize>;

    fn length(&self) -> u32;
    fn signature(&self) -> u32;
}
impl_downcast!(sync AcpiInstallable);

#[repr(C)]
#[derive(Default, Clone)]
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
    pub table_key: TableKey,   // Unique key assigned to ACPI table upon installation
    pub versions: AcpiVersion, // Bitflag of versions this table is installed for
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
