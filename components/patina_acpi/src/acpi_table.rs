//! ACPI Table Definitions.
//!
//! Defines standard formats for system ACPI tables.
//! Supports only ACPI version >= 2.0.
//! Fields corresponding to ACPI 1.0 are preceded with an underscore (`_`) and are not in use.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent

use alloc::boxed::Box;
use alloc::vec::Vec;
use patina_sdk::component::service::memory::MemoryManager;
use patina_sdk::component::service::Service;
use patina_sdk::efi_types::EfiMemoryType;

use crate::error::AcpiError;
use crate::signature::{self};

use core::mem::ManuallyDrop;
use core::ptr::{self, NonNull};
use core::{mem, slice};

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

/// Represents the XSDT for ACPI 2.0+.
/// The XSDT has a standard header followed by 64-bit addresses of installed tables.
/// The `length` field of the header tells us the number of trailing bytes representing table entries.
#[repr(C)]
#[derive(Default, Copy, Clone)]
pub struct AcpiXsdt {
    pub(crate) header: AcpiTableHeader,
}

/// Stores implementation-specific data about the XSDT.
pub(crate) struct AcpiXsdtMetadata {
    pub(crate) n_entries: usize,
    pub(crate) max_capacity: usize,
    pub(crate) slice: Box<[u8], &'static dyn alloc::alloc::Allocator>,
}

impl AcpiXsdtMetadata {
    // Get the 4-byte length (bytes 4..8 of the header).
    pub(crate) fn get_length(&self) -> Result<u32, AcpiError> {
        // XSDT always starts with header.
        let length_offset = mem::offset_of!(AcpiTableHeader, length);
        // Grab the current length from the correct offset in the header.
        self.slice
            .get(length_offset..length_offset + mem::size_of::<u32>()) // Length is a u32
            .and_then(|b| b.try_into().ok())
            .map(u32::from_le_bytes)
            .ok_or(AcpiError::XsdtOverflow)
    }

    // Set the 4-byte length (bytes 4..8 of the header).
    pub(crate) fn set_length(&mut self, new_len: u32) {
        // XSDT always starts with header.
        let length_offset = mem::offset_of!(AcpiTableHeader, length);
        // Write the new length into the correct offset in the header.
        self.slice[length_offset..length_offset + mem::size_of::<u32>()] // Length is a u32
            .copy_from_slice(&new_len.to_le_bytes());
    }

    /// Set the 6-byte OEM ID (bytes 10..16 of the header).
    pub(crate) fn set_oem_id(&mut self, new_id: [u8; 6]) {
        let offset = mem::offset_of!(AcpiTableHeader, oem_id);
        let end = offset + mem::size_of::<[u8; 6]>();
        self.slice[offset..end].copy_from_slice(&new_id);
    }

    /// Set the 8-byte OEM Table ID (bytes 16..24 of the header).
    pub(crate) fn set_oem_table_id(&mut self, new_table_id: [u8; 8]) {
        let offset = mem::offset_of!(AcpiTableHeader, oem_table_id);
        let end = offset + mem::size_of::<[u8; 8]>();
        self.slice[offset..end].copy_from_slice(&new_table_id);
    }

    /// Set the 4-byte OEM Revision (bytes 24..28 of the header).
    pub(crate) fn set_oem_revision(&mut self, new_rev: u32) {
        let offset = mem::offset_of!(AcpiTableHeader, oem_revision);
        let end = offset + mem::size_of::<u32>();
        self.slice[offset..end].copy_from_slice(&new_rev.to_le_bytes());
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

impl AcpiTableHeader {
    /// Serialize `self` into a `Vec<u8>` in ACPI's canonical layout.
    pub fn hdr_to_bytes(&self) -> Vec<u8> {
        // Pre‑allocate exactly the right length
        let mut buf = Vec::with_capacity(mem::size_of::<Self>());

        // Signature (4 bytes)
        buf.extend_from_slice(&self.signature.to_le_bytes());

        // Length (4 bytes, little‑endian)
        buf.extend_from_slice(&self.length.to_le_bytes());

        // Revision (1 byte), Checksum (1 byte)
        buf.push(self.revision);
        buf.push(self.checksum);

        // OEM ID (6 bytes)
        buf.extend_from_slice(&self.oem_id);

        // OEM Table ID (8 bytes)
        buf.extend_from_slice(&self.oem_table_id);

        // OEM Revision (4 bytes, little‑endian)
        buf.extend_from_slice(&self.oem_revision.to_le_bytes());

        // Creator ID (4 bytes, little‑endian)
        buf.extend_from_slice(&self.creator_id.to_le_bytes());

        // Creator Revision (4 bytes, little‑endian)
        buf.extend_from_slice(&self.creator_revision.to_le_bytes());

        buf
    }
}

/// The inner table structure.
pub(crate) union Table<T = AcpiTableHeader> {
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
    pub unsafe fn new(table: T) -> Result<Self, AcpiError> {
        let returned_table = Table { inner: ManuallyDrop::new(table) };

        // Make sure all bytes are valid ASCII.
        // By spec, ACPI table signatures are length-4 ASCII strings (represented numerically as u32's).
        let is_valid_ascii = returned_table.signature().to_le_bytes().iter().all(|b| b.is_ascii());
        if !is_valid_ascii {
            return Err(AcpiError::InvalidTableFormat);
        }

        // Make sure length is valid for type T.
        if (returned_table.header.length as usize) < mem::size_of::<T>() {
            return Err(AcpiError::InvalidTableFormat);
        }

        Ok(returned_table)
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

#[derive(Clone, Copy, Debug)]
pub struct AcpiTable {
    pub(crate) table: NonNull<Table>,
}

impl AcpiTable {
    /// Creates a new AcpiTable from a given table.
    /// ## Safety
    ///
    /// - Caller must ensure the provided table, `T`, has a C compatible layout (typically using `#[repr(C)]`).
    /// - Caller must ensure that the table's first field is [AcpiTableHeader].
    pub unsafe fn new<T>(table: T, mm: &Service<dyn MemoryManager>) -> Result<Self, AcpiError> {
        let table = Table::new(table)?;

        // FACS and UEFI tables must always be located in NVS (by spec).
        let allocator_type = match table.signature() {
            signature::FACS | signature::UEFI => EfiMemoryType::ACPIMemoryNVS,
            _ => EfiMemoryType::ACPIReclaimMemory,
        };

        let table = NonNull::from(Box::leak(Box::new_in(
            table,
            mm.get_allocator(allocator_type).map_err(|_e| AcpiError::AllocationFailed)?,
        )))
        .cast::<Table>();

        Ok(AcpiTable { table })
    }

    /// Creates a new AcpiTable from a raw pointer.
    /// When created this way, the type of the table is unknown.
    ///
    /// ## Safety
    ///
    /// - Caller must ensure the pointer refers to a valid ACPI table.
    /// - Caller must ensure `table_length` is correctly specifies the length of the table, including the header and any trailing data bytes.
    pub unsafe fn new_from_ptr(
        header_ptr: *const AcpiTableHeader,
        mm: &Service<dyn MemoryManager>,
    ) -> Result<Self, AcpiError> {
        let table_signature = (*header_ptr).signature;
        let table_length = (*header_ptr).length as usize;

        let allocator_type = match table_signature {
            signature::FACS | signature::UEFI => EfiMemoryType::ACPIMemoryNVS,
            _ => EfiMemoryType::ACPIReclaimMemory,
        };

        // Allocate table based on the length given in the header field.
        let allocator = mm.get_allocator(allocator_type).map_err(|_e| AcpiError::AllocationFailed)?;
        let mut table_bytes = Vec::with_capacity_in(table_length, allocator);

        // Copy entire table into the new allocation.
        ptr::copy_nonoverlapping(header_ptr as *const u8, table_bytes.as_mut_ptr(), table_length);
        table_bytes.set_len(table_length);

        // Leak the allocated bytes.
        let raw_header = Box::into_raw(table_bytes.into_boxed_slice()) as *mut AcpiTableHeader;

        Ok(Self { table: NonNull::new(raw_header).ok_or(AcpiError::NullTablePtr)?.cast::<Table>() })
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

    /// Returns a raw byte slice over the entire table.
    ///
    /// ## SAFETY
    /// `self.length` must accurately reflect the allocated size of the table.
    pub unsafe fn as_bytes(&self) -> &[u8] {
        unsafe { slice::from_raw_parts(self.table.as_ptr() as *const u8, self.header().length as usize) }
    }

    /// Returns a mutable byte slice over the entire table.
    /// (This is primarily useful for computing the checksum.)
    ///
    /// ## SAFETY
    /// `self.length` must accurately reflect the allocated size of the table.
    pub unsafe fn as_bytes_mut(&mut self) -> &mut [u8] {
        unsafe { slice::from_raw_parts_mut(self.table.as_ptr() as *mut u8, self.header().length as usize) }
    }

    /// Updates the checksum for an ACPI table.
    /// According to the ACPI spec 2.0+, all bytes of a table must sum to zero modulo 256.
    pub fn update_checksum(&mut self, offset: usize) -> Result<(), AcpiError> {
        let bytes = unsafe { self.as_bytes_mut() };
        let len = bytes.len();

        // Set the checksum field (byte at the specified `offset`) to zero before recalculation.
        if len > offset {
            bytes[offset] = 0;

            // Recalculate checksum and set so that total sum is 0.
            let sum: u8 = bytes.iter().fold(0u8, |sum, &b| sum.wrapping_add(b));
            bytes[offset] = (0u8).wrapping_sub(sum);
            Ok(())
        } else {
            Err(AcpiError::InvalidChecksumOffset)
        }
    }

    /// Returns a reference to the entire AcpiTable.
    ///
    /// ## Safety
    ///
    /// - Caller must ensure that the provided table format is the same as `T`.
    pub unsafe fn as_ref<T>(&self) -> &T {
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

#[cfg(test)]
mod tests {
    use patina_sdk::component::service::memory::StdMemoryManager;

    use crate::signature::ACPI_CHECKSUM_OFFSET;

    use super::*;
    use core::mem;
    use core::ptr::NonNull;

    #[repr(C)]
    struct TestTable {
        header: AcpiTableHeader,
        body: [u8; 3],
    }

    const TEST_SIGNATURE: u32 = 0x123;

    #[test]
    fn test_update_checksum_on_real_acpi_table() {
        // Build a mock table.
        let test_table = TestTable {
            header: AcpiTableHeader {
                signature: TEST_SIGNATURE,
                length: (mem::size_of::<TestTable>()) as u32,
                revision: 1,
                checksum: 0, // we'll fill this
                oem_id: [0; 6],
                oem_table_id: *b"TBL_ID__",
                oem_revision: 0xAABBCCDD,
                creator_id: 0x11223344,
                creator_revision: 0x55667788,
            },
            body: [10, 20, 30], // some payload bytes
        };

        // Set up the test table.
        let table_union: Table<TestTable> = unsafe { Table::new(test_table).unwrap() };
        // Box it on the heap (uses the global allocator).
        let boxed: Box<Table<TestTable>> = Box::new(table_union);
        let raw_ptr: *mut Table<TestTable> = Box::into_raw(boxed);
        let nn = unsafe { NonNull::new_unchecked(raw_ptr as *mut Table) };

        // Wrap in AcpiTable.
        let mut acpi_table = AcpiTable { table: nn };

        // Update the checksum (use standard checksum offset since it has a standard header).
        let offset = ACPI_CHECKSUM_OFFSET;
        assert!(acpi_table.update_checksum(offset).is_ok());

        // Pull out the bytes and verify the checksum.
        let bytes: &[u8] = unsafe { acpi_table.as_bytes() };
        // Total sum must be zero mod 256.
        let total: u8 = bytes.iter().copied().fold(0u8, |acc, b| acc.wrapping_add(b));
        assert_eq!(total, 0, "entire table did not sum to zero");
    }

    #[test]
    fn test_new_from_ptr_creates_valid_acpi_table() {
        // Build a mock table.
        let test_table = TestTable {
            header: AcpiTableHeader {
                signature: TEST_SIGNATURE,
                length: (mem::size_of::<TestTable>()) as u32,
                revision: 2,
                checksum: 0,
                oem_id: [1, 2, 3, 4, 5, 6],
                oem_table_id: *b"test_tes",
                oem_revision: 0xDEADBEEF,
                creator_id: 0xCAFEBABE,
                creator_revision: 0xFEEDFACE,
            },
            body: [42, 43, 44],
        };

        // Allocate the table on the heap.
        let boxed = Box::new(test_table);
        let raw_ptr = Box::into_raw(boxed);

        let mm: Service<dyn MemoryManager> = Service::mock(Box::new(StdMemoryManager::new()));

        // SAFETY: raw_ptr points to a valid TestTable with a valid header.
        let acpi_table = unsafe { AcpiTable::new_from_ptr(raw_ptr as *const AcpiTableHeader, &mm) }.unwrap();

        // Check signature and header fields.
        assert_eq!(acpi_table.signature(), TEST_SIGNATURE);
        let header = acpi_table.header();
        assert_eq!(header.length, mem::size_of::<TestTable>() as u32);
        assert_eq!(header.revision, 2);
        assert_eq!(header.oem_id, [1, 2, 3, 4, 5, 6]);
        assert_eq!(header.oem_table_id, *b"test_tes");
        assert_eq!(header.oem_revision, 0xDEADBEEF);
        assert_eq!(header.creator_id, 0xCAFEBABE);
        assert_eq!(header.creator_revision, 0xFEEDFACE);

        // Check that the body bytes are correct.
        let bytes = unsafe { acpi_table.as_bytes() };
        let body_offset = mem::size_of::<AcpiTableHeader>();
        assert_eq!(&bytes[body_offset..body_offset + 3], &[42, 43, 44]);
    }
}
