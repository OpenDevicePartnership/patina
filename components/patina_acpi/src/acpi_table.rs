//! ACPI Table Definitions.
//!
//! Defines standard formats for system ACPI tables.
//! Supports only ACPI version >= 2.0.
//! Fields corresponding to ACPI 1.0 are preceded with an underscore (`_`) and are not in use.

use alloc::boxed::Box;
use downcast_rs::{impl_downcast, Downcast};
use patina_sdk::component::service::memory::MemoryManager;
use patina_sdk::component::service::Service;
use patina_sdk::efi_types::EfiMemoryType;

use crate::error::AcpiError;
use crate::{service::TableKey, signature::ACPI_HEADER_LEN};

use core::any::{Any, TypeId};
use core::mem::ManuallyDrop;
use core::ptr::NonNull;
use core::{mem, slice};

/// Any ACPI table with the standard ACPI header.
pub trait StandardAcpiTable: Any + Downcast {
    /// The standard 36-byte ACPI header.
    fn header(&self) -> &AcpiTableHeader;

    /// Return the entire table as a &[u8], based on the header.length.
    fn as_bytes(&self) -> &[u8] {
        let header = self.header();
        let length = header.length as usize;
        let ptr = header as *const AcpiTableHeader as *const u8;
        // SAFETY: we trust that the ACPI table in memory is valid for `length` bytes
        unsafe { slice::from_raw_parts(ptr, length) }
    }

    /// Return the entire table as a mutable &[u8], based on the header.length.
    fn as_bytes_mut(&mut self) -> &mut [u8] {
        let header = self.header();
        let length = header.length as usize;
        let ptr = header as *const AcpiTableHeader as *mut u8;
        // SAFETY: we trust that the ACPI table in memory is valid for `length` bytes
        unsafe { slice::from_raw_parts_mut(ptr, length) }
    }
}

impl_downcast!(StandardAcpiTable);

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
/// The FACS is always allocated in NVS, and is required to be 64B-aligned.
/// Equivalent to EFI_ACPI_3_0_FIRMWARE_ACPI_CONTROL_STRUCTURE.
#[repr(C, align(64))]
#[derive(Default, Clone, Copy)]
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
#[derive(Default, Copy, Clone)]
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

impl AcpiRsdp {
    /// Borrowed view of the raw bytes.
    pub fn as_bytes_mut(&mut self) -> &mut [u8] {
        // SAFETY: `&self` is valid for reads of exactly `size` bytes,
        // and #[repr(C, packed)] guarantees no hidden padding.
        unsafe { slice::from_raw_parts_mut((self as *mut Self).cast::<u8>(), self.length as usize) }
    }
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

/// Represents a raw pointer to an ACPI table in C.
/// Because the table is abstracted as a pointer, the `type_id` may not be valid.
pub struct RawAcpiTable {
    header: NonNull<AcpiTableHeader>,
}

impl RawAcpiTable {
    /// Converts an address to a `CAcpiTable`.
    ///
    /// # Safety
    /// The caller must ensure that the address is in valid ACPI memory and points to a valid ACPI table.
    pub fn new_from_address(addr: u64) -> Result<Self, AcpiError> {
        let header = addr as *mut AcpiTableHeader;
        Ok(Self { header: NonNull::new(header).ok_or(AcpiError::NullTablePtr)? })
    }
}

impl StandardAcpiTable for RawAcpiTable {
    /// # Safety
    /// The caller must ensure that the address is in valid ACPI memory and points to a valid ACPI table.
    fn header(&self) -> &AcpiTableHeader {
        // SAFETY: The first field of any ACPI table is the header.
        unsafe { self.header.as_ref() }
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

/// The inner table structure.
union Table<T = AcpiTableHeader> {
    /// The signature of the ACPI table.
    signature: u32,
    /// The header of the ACPI table.
    header: AcpiTableHeader,
    /// The full ACPI table, represented as its original type.
    inner: ManuallyDrop<T>,
}

impl<T> Table<T> {
    /// Creates a new table.
    ///
    /// ## Safety
    ///
    /// - Caller must ensure the provided table, `T`, has a C compatible layout (typically using `#[repr(C)]`).
    /// - Caller must ensure that the table's first field is [AcpiTableHeader].
    pub unsafe fn new(table: T) -> Self {
        Table { inner: ManuallyDrop::new(table) }
    }

    /// Returns the signature of the ACPI table.
    pub fn signature(&self) -> u32 {
        // SAFETY: [Self::new] ensures that the first field is a u32.
        unsafe { self.signature }
    }

    /// Returns an immutable reference to the entire table.
    pub fn as_ref(&self) -> &T {
        // SAFETY: [Self::new] insures the inner object is a valid instance of `T`.
        unsafe { &self.inner }
    }

    /// Returns an immutable reference to the entire table.
    pub fn as_mut(&mut self) -> &mut T {
        // SAFETY: [Self::new] insures the inner object is a valid instance of `T`.
        unsafe { &mut self.inner }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct AcpiTable {
    table: NonNull<Table>,
}

impl AcpiTable {
    pub const FACS: u32 = 0x53434146;
    pub const UEFI: u32 = 0x49464555;
    pub const FACP: u32 = 0x50434146;
    pub const FADT: u32 = AcpiTable::FACP;
    pub const DSDT: u32 = 0x54534444;
    pub const XSDT: u32 = 0x54445358;

    /// Creates a new AcpiTable from a given table.
    pub unsafe fn new<T>(table: T, mm: &Service<dyn MemoryManager>) -> Self {
        let table = Table::new(table);

        // FACS and UEFI tables must always be located in NVS (by spec).
        let allocator_type = match table.signature() {
            Self::FACS | Self::UEFI => EfiMemoryType::ACPIMemoryNVS,
            _ => EfiMemoryType::ACPIReclaimMemory,
        };

        let table =
            NonNull::from(Box::leak(Box::new_in(table, mm.get_allocator(allocator_type).unwrap()))).cast::<Table>();

        AcpiTable { table }
    }

    pub fn signature(&self) -> u32 {
        // SAFETY: The table is guaranteed to be a valid ACPI table.
        unsafe { self.table.as_ref().signature() }
    }

    pub fn header(&self) -> &AcpiTableHeader {
        // SAFETY: The table is guaranteed to be a valid ACPI table.
        unsafe { &self.table.as_ref().header }
    }

    pub fn header_mut(&mut self) -> &mut AcpiTableHeader {
        // SAFETY: The table is guaranteed to be a valid ACPI table.
        unsafe { &mut self.table.as_mut().header }
    }

    pub fn update_checksum(&mut self) {
        todo!()
    }

    /// Returns a reference to the entire AcpiTable.
    ///
    /// ## Safety
    ///
    /// - Caller must ensure that the provided table format is the same as `T`.
    pub fn as_ref<T>(&self) -> &T {
        // SAFETY: Caller must ensure that the provided table format is the same as `T`.
        unsafe { self.table.cast::<Table<T>>().as_ref().as_ref() }
    }

    /// Returns a mutable reference to the entire AcpiTable.
    ///
    /// ## Safety
    ///
    /// - Caller must ensure that the provided table format is the same as `T`.
    pub unsafe fn as_mut<T>(&mut self) -> &mut T {
        // SAFETY: Caller must ensure that the provided table format is the same as `T`.
        unsafe { self.table.cast::<Table<T>>().as_mut().as_mut() }
    }

    /// Returns a pointer the the underlying AcpiTable.
    pub fn as_ptr(&self) -> *const AcpiTableHeader {
        self.table.as_ptr() as *const AcpiTableHeader
    }

    /// Returns a mutable pointer the the underlying AcpiTable.
    pub fn as_mut_ptr(&self) -> *mut AcpiTableHeader {
        self.table.as_ptr() as *mut AcpiTableHeader
    }
}
