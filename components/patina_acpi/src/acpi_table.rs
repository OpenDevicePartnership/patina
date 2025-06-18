//! ACPI Table Definitions.
//!
//! Defines standard formats for system ACPI tables.
//! Supports only ACPI version >= 2.0.
//! Fields corresponding to ACPI 1.0 are preceded with an underscore (`_`) and are not in use.

use crate::error::AcpiError;
use crate::{service::TableKey, signature::ACPI_HEADER_LEN};

use core::any::{Any, TypeId};
use core::ptr::NonNull;
use core::slice;

/// Any ACPI table with the standard ACPI header.
pub trait StandardAcpiTable: Any {
    /// The standard 36-byte ACPI header.
    fn header(&self) -> &AcpiTableHeader;
}

/// Represents the FADT for ACPI 2.0+.
/// Equivalent to EFI_ACPI_3_0_FIXED_ACPI_DESCRIPTION_TABLE.
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub(crate) struct AcpiFadt {
    // Standard ACPI header.
    pub(crate) header: AcpiTableHeader,
    pub(crate) inner: FadtData,
}

#[repr(C, packed)]
#[derive(Default, Clone, Copy)]
pub(crate) struct FadtData {
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

impl StandardAcpiTable for AcpiFadt {
    fn header(&self) -> &AcpiTableHeader {
        &self.header
    }
}

/// Reads unaligned fields on the FADT.
/// Fields on the FADT may be unaligned, since by specification the FADT is packed.
impl AcpiFadt {
    pub(crate) fn x_firmware_ctrl(&self) -> u64 {
        self.inner.x_firmware_ctrl
    }

    pub(crate) fn x_dsdt(&self) -> u64 {
        self.inner.x_dsdt
    }

    pub(crate) fn set_x_firmware_ctrl(&mut self, address: u64) {
        self.inner.x_firmware_ctrl = address;
    }

    pub(crate) fn set_x_dsdt(&mut self, address: u64) {
        self.inner.x_dsdt = address;
    }
}

/// Represents the FACS for ACPI 2.0+.
/// Note that the FACS does not have a standard ACPI header.
/// The FACS is not present in the list of installed ACPI tables; instead, it is only accessible through the FADT's `x_firmware_ctrl` field.
/// Equivalent to EFI_ACPI_3_0_FIRMWARE_ACPI_CONTROL_STRUCTURE.
#[repr(C)]
#[derive(Default)]
pub struct AcpiFacs {
    pub(crate) signature: u32,
    pub(crate) length: u32,
    pub(crate) hardware_signature: u32,

    pub(crate) _firmware_waking_vector: u32,

    pub(crate) global_lock: u32,
    pub(crate) flags: u32,
    pub(crate) x_firmware_waking_vector: u64,
    pub(crate) version: u8,
    pub(crate) reserved: [u8; 31],
}

/// Represents the DSDT for ACPI 2.0+.
/// The DSDT is not present in the list of installed ACPI tables; instead, it is only accessible through the FADT's `x_dsdt` field.
/// The DSDT has a standard header followed by variable-length AML bytecode.
/// The `length` field of the header tells us the number of trailing bytes representing bytecode.
#[repr(C)]
#[derive(Default)]
pub struct AcpiDsdt {
    pub(crate) header: AcpiTableHeader,
}

impl StandardAcpiTable for AcpiDsdt {
    fn header(&self) -> &AcpiTableHeader {
        &self.header
    }
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
    pub(crate) header: AcpiTableHeader,
}

impl StandardAcpiTable for AcpiXsdt {
    fn header(&self) -> &AcpiTableHeader {
        &self.header
    }
}

/// Represents a raw to an ACPI table in C.
/// Because the table is abstracted as a pointer, the `type_id` may not be valid.
pub struct RawAcpiTable {
    address: u64,
}

impl RawAcpiTable {
    /// Converts an address to a `CAcpiTable`.
    ///
    /// # Safety
    /// The caller must ensure that the address is in valid ACPI memory and points to a valid ACPI table.
    unsafe fn from_address(addr: u64) -> Self {
        Self { address: addr }
    }
}

impl StandardAcpiTable for RawAcpiTable {
    /// # Safety
    /// The caller must ensure that the address is in valid ACPI memory and points to a valid ACPI table.
    fn header(&self) -> &AcpiTableHeader {
        // SAFETY: The first field of any ACPI table is the header.
        unsafe { &*(self.address as *const AcpiTableHeader) }
    }
}

/// Represents a standard ACPI header.
/// Equivalent to EFI_ACPI_DESCRIPTION_HEADER.
#[repr(C)]
#[derive(Default, Clone, Debug, Copy)]
pub struct AcpiTableHeader {
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

/// Represents an ACPI table installed in memory, including a header and any trailing bytes.
/// Here, the trailing bytes are copied to the heap and owned by the struct.
#[derive(Clone, Debug)]
pub struct MemoryAcpiTable {
    /// Standard ACPI header.
    pub header: NonNull<AcpiTableHeader>,

    /* The following fields are not present in the ACPI specification, but are included for implementation convenience. */
    /// Type ID of the table, used to identify the specific type of ACPI table.
    pub type_id: TypeId,
    /// Unique key assigned to ACPI table upon installation. Zero if the table does not have an associated key.
    pub table_key: TableKey,
    /// Physical address of the table in memory. None if the table is not yet installed in ACPI firmware memory.
    pub physical_address: Option<usize>,
}

impl MemoryAcpiTable {
    /// Creates a new ACPI table from a pointer.
    /// Since the type is abstracted by the raw pointer, the `type_id` field will not be valid.
    pub fn new_from_ptr(header: *mut AcpiTableHeader) -> Result<Self, AcpiError> {
        let nonnull_header = NonNull::new(header).ok_or(AcpiError::NullTablePtr)?;
        Ok(MemoryAcpiTable {
            header: nonnull_header,
            type_id: TypeId::of::<MemoryAcpiTable>(),
            table_key: 0,
            physical_address: None,
        })
    }
}

/// Implementations to access the fields of the ACPI table header.
/// SAFETY: Pointer has been checked as non-null and can only be a AcpiTable
impl MemoryAcpiTable {
    /// Revision number of the table.
    pub fn revision(&self) -> u8 {
        unsafe { self.header.as_ref() }.revision
    }

    /// Header checksum (all bytes in the table must sum to zero).
    pub fn checksum(&self) -> u8 {
        unsafe { self.header.as_ref() }.checksum
    }

    /// OEM ID (6 ASCII characters, not null-terminated).
    pub fn oem_id(&self) -> [u8; 6] {
        unsafe { self.header.as_ref() }.oem_id
    }

    /// OEM Table ID (8 ASCII characters, not null-terminated).
    pub fn oem_table_id(&self) -> [u8; 8] {
        unsafe { self.header.as_ref() }.oem_table_id
    }

    /// OEM revision number.
    pub fn oem_revision(&self) -> u32 {
        unsafe { self.header.as_ref() }.oem_revision
    }

    /// Creator ID (often the compiler/vendor signature).
    pub fn creator_id(&self) -> u32 {
        unsafe { self.header.as_ref() }.creator_id
    }

    /// Creator revision number.
    pub fn creator_revision(&self) -> u32 {
        unsafe { self.header.as_ref() }.creator_revision
    }

    /// Total table length, including header and any trailing fields.
    pub fn length(&self) -> u32 {
        unsafe { self.header.as_ref() }.length
    }

    /// Table signature, which defines what type of table it is.
    pub fn signature(&self) -> u32 {
        unsafe { self.header.as_ref() }.signature
    }

    /// Retrieves the header mutably.
    pub fn header_mut(&mut self) -> &mut AcpiTableHeader {
        unsafe { self.header.as_mut() }
    }

    /// Variable-length trailing bytes of the ACPI table, following the header.
    /// # Safety
    /// During construction, it is assumed that the table is laid out as a contiguous array of bytes following the ACPI specification.
    pub fn data(&self) -> &[u8] {
        let inmemory_header = self.header.as_ptr();
        let data_start_ptr = unsafe { (inmemory_header as *const u8).add(ACPI_HEADER_LEN) };
        let data_len = self.length() as usize - ACPI_HEADER_LEN;
        unsafe { slice::from_raw_parts(data_start_ptr, data_len) }
    }

    /// Variable-length trailing bytes of the ACPI table, following the header.
    /// # Safety
    /// During construction, it is assumed that the table is laid out as a contiguous array of bytes following the ACPI specification.
    pub fn data_mut(&mut self) -> &mut [u8] {
        let inmemory_header = self.header.as_ptr();
        let data_start_ptr = unsafe { (inmemory_header as *mut u8).add(ACPI_HEADER_LEN) };
        let data_len = self.length() as usize - ACPI_HEADER_LEN;
        unsafe { slice::from_raw_parts_mut(data_start_ptr, data_len) }
    }

    /// The ACPI table as a byte slice, including the header and trailing bytes.
    /// # Safety
    /// During construction, it is assumed that the table is laid out as a contiguous array of bytes following the ACPI specification.
    pub fn as_mut<T: 'static>(&mut self) -> Option<&mut T> {
        if self.type_id == TypeId::of::<T>() {
            // SAFETY: The type ID's match, so it is castable to this type.
            Some(unsafe { self.header.cast::<T>().as_mut() })
        } else {
            None
        }
    }
}
