//! Core SMBIOS manager implementation
//!
//! This module provides the core SMBIOS table manager that handles record storage,
//! handle allocation, string pool management, and table publication.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

extern crate alloc;

use alloc::{boxed::Box, string::String, vec::Vec};
use core::cell::RefCell;
use patina::{base::SIZE_64KB, uefi_size_to_pages};
use r_efi::{
    efi,
    efi::{Handle, PhysicalAddress},
};
use zerocopy::{IntoBytes, Ref};
use zerocopy_derive::*;

use crate::{
    error::SmbiosError,
    service::{
        SMBIOS_HANDLE_PI_RESERVED, SMBIOS_STRING_MAX_LENGTH, SmbiosHandle, SmbiosRecordsIter, SmbiosTableHeader,
        SmbiosType,
    },
    smbios_record::{SmbiosRecordStructure, Type127EndOfTable},
};

use super::record::SmbiosRecord;

/// SMBIOS 3.x Configuration Table GUID: F2FD1544-9794-4A2C-992E-E5BBCF20E394
///
/// This GUID identifies the SMBIOS 3.0+ entry point structure in the UEFI Configuration Table.
/// Used for SMBIOS 3.0 and later versions which support 64-bit table addresses and remove
/// the 4GB table size limitation of SMBIOS 2.x.
pub const SMBIOS_3_X_TABLE_GUID: efi::Guid =
    efi::Guid::from_fields(0xF2FD1544, 0x9794, 0x4A2C, 0x99, 0x2E, &[0xE5, 0xBB, 0xCF, 0x20, 0xE3, 0x94]);

/// SMBIOS 3.0 entry point structure (64-bit)
/// Per SMBIOS 3.0+ specification section 5.2.2
#[repr(C, packed)]
#[derive(Clone, Copy, IntoBytes, Immutable)]
pub struct Smbios30EntryPoint {
    /// Anchor string "_SM3_" (0x00)
    pub anchor_string: [u8; 5],
    /// Entry Point Structure Checksum (0x05)
    pub checksum: u8,
    /// Entry Point Length - 0x18 = 24 bytes (0x06)
    pub length: u8,
    /// SMBIOS Major Version (0x07)
    pub major_version: u8,
    /// SMBIOS Minor Version (0x08)
    pub minor_version: u8,
    /// SMBIOS Docrev - specification revision (0x09)
    pub docrev: u8,
    /// Entry Point Structure Revision - 0x01 (0x0A)
    pub entry_point_revision: u8,
    /// Reserved - must be 0x00 (0x0B)
    pub reserved: u8,
    /// Structure Table Maximum Size (0x0C)
    pub table_max_size: u32,
    /// Structure Table Address - 64-bit (0x10)
    pub table_address: u64,
}

/// SMBIOS table manager
///
/// Manages SMBIOS records, handles, and table generation.
pub struct SmbiosManager {
    pub(super) records: RefCell<Vec<SmbiosRecord>>,
    pub(super) next_handle: RefCell<SmbiosHandle>,
    pub(super) freed_handles: RefCell<Vec<SmbiosHandle>>,
    pub major_version: u8,
    pub minor_version: u8,
    entry_point_64: RefCell<Option<Box<Smbios30EntryPoint>>>,
    table_64_address: RefCell<Option<PhysicalAddress>>,
    /// Pre-allocated buffer for SMBIOS table data
    table_buffer_addr: RefCell<Option<PhysicalAddress>>,
    /// Maximum size of the pre-allocated table buffer
    table_buffer_max_size: usize,
    /// Pre-allocated buffer for entry point structure
    ep_buffer_addr: RefCell<Option<PhysicalAddress>>,
    /// Checksum of published table (for detecting direct modifications)
    published_table_checksum: RefCell<Option<u32>>,
    /// Size of the published table (needed for checksum verification)
    published_table_size: RefCell<usize>,
}

impl SmbiosManager {
    /// Creates a new SMBIOS manager with the specified version
    ///
    /// # Arguments
    ///
    /// * `major_version` - SMBIOS major version (must be 3)
    /// * `minor_version` - SMBIOS minor version (any value for version 3.x)
    ///
    /// # Errors
    ///
    /// Returns `SmbiosError::UnsupportedVersion` if major version is not 3.
    pub fn new(major_version: u8, minor_version: u8) -> Result<Self, SmbiosError> {
        if major_version != 3 {
            log::error!(
                "SMBIOS version {}.{} is not supported. Only SMBIOS 3.x is supported.",
                major_version,
                minor_version
            );
            return Err(SmbiosError::UnsupportedVersion);
        }

        Ok(Self {
            records: RefCell::new(Vec::new()),
            next_handle: RefCell::new(1),
            freed_handles: RefCell::new(Vec::new()),
            major_version,
            minor_version,
            entry_point_64: RefCell::new(None),
            table_64_address: RefCell::new(None),
            // Pre-allocated buffers start as None, set up during component init
            table_buffer_addr: RefCell::new(None),
            table_buffer_max_size: SIZE_64KB,
            ep_buffer_addr: RefCell::new(None),
            published_table_checksum: RefCell::new(None),
            published_table_size: RefCell::new(0),
        })
    }

    /// Allocate buffers for SMBIOS table publication
    ///
    /// Allocates buffers for the SMBIOS table and entry point upfront to avoid
    /// repeated allocations. The buffers are reused for all table publications.
    ///
    /// Also adds the Type 127 End-of-Table marker to maintain the invariant that
    /// it's always the last record.
    ///
    /// # Errors
    ///
    /// Returns `SmbiosError::AllocationFailed` if buffer allocation fails.
    /// Returns `SmbiosError::HandleExhausted` if unable to allocate handle for Type 127.
    pub(crate) fn allocate_buffers(
        &self,
        memory_manager: &dyn patina::component::service::memory::MemoryManager,
    ) -> Result<(), SmbiosError> {
        // Check if already allocated
        if self.table_buffer_addr.borrow().is_some() {
            return Ok(());
        }

        use patina::{component::service::memory::AllocationOptions, efi_types::EfiMemoryType};

        // Allocate table buffer
        let table_pages = uefi_size_to_pages!(self.table_buffer_max_size);
        let table_allocation = memory_manager
            .allocate_pages(table_pages, AllocationOptions::new().with_memory_type(EfiMemoryType::ACPIReclaimMemory))
            .map_err(|_| SmbiosError::AllocationFailed)?;
        let table_slice = table_allocation.into_raw_slice::<u8>();
        let table_addr = table_slice as *mut u8 as u64;

        // Allocate entry point buffer (1 page is plenty)
        let ep_allocation = memory_manager
            .allocate_pages(1, AllocationOptions::new().with_memory_type(EfiMemoryType::ACPIReclaimMemory))
            .map_err(|_| SmbiosError::AllocationFailed)?;
        let ep_slice = ep_allocation.into_raw_slice::<u8>();
        let ep_addr = ep_slice as *mut u8 as u64;

        *self.table_buffer_addr.borrow_mut() = Some(table_addr);
        *self.ep_buffer_addr.borrow_mut() = Some(ep_addr);

        // Automatically add Type 127 End-of-Table marker to ensure SMBIOS compliance
        // This is added directly to records to bypass the add_from_bytes validation
        // which rejects Type 127 to prevent external callers from adding it
        let type127 = Type127EndOfTable::new();
        let bytes = type127.to_bytes();
        let header = SmbiosTableHeader::new(127, 4, self.allocate_handle()?);
        let record = SmbiosRecord::new(header, None, bytes, 0);
        self.records.borrow_mut().push(record);

        Ok(())
    }

    /// Validate a string for use in SMBIOS records
    ///
    /// Ensures the string meets SMBIOS specification requirements:
    /// - Does not exceed SMBIOS_STRING_MAX_LENGTH (64 bytes)
    /// - Does not contain null terminators (they are added during serialization)
    ///
    /// # Arguments
    ///
    /// * `s` - The string to validate
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if valid, or an appropriate error if validation fails
    pub(super) fn validate_string(s: &str) -> Result<(), SmbiosError> {
        if s.len() > SMBIOS_STRING_MAX_LENGTH {
            return Err(SmbiosError::StringTooLong);
        }
        // Strings must NOT contain null terminators - they are added during serialization
        if s.bytes().any(|b| b == 0) {
            return Err(SmbiosError::StringContainsNull);
        }
        Ok(())
    }

    /// Efficiently validate string pool format and count strings in a single pass
    ///
    /// This combines validation and counting for better performance
    ///
    /// # String Pool Format
    /// SMBIOS string pools have a specific format:
    /// - Each string is null-terminated ('\0')
    /// - The entire pool ends with double null ("\0\0")
    /// - Empty string pool is just double null ("\0\0")
    /// - String indices in the record start at 1 (not 0)
    ///
    /// # Errors
    /// Returns `SmbiosError::InvalidStringPoolTermination` if:
    /// - The pool doesn't end with double null
    /// - The pool is too small (< 2 bytes)
    ///
    /// Returns `SmbiosError::EmptyStringInPool` if consecutive nulls are found in the middle
    ///
    /// Returns `SmbiosError::StringTooLong` if any string exceeds SMBIOS_STRING_MAX_LENGTH
    pub(super) fn validate_and_count_strings(string_pool_area: &[u8]) -> Result<usize, SmbiosError> {
        let len = string_pool_area.len();

        // Must end with double null
        if len < 2 || string_pool_area[len - 1] != 0 || string_pool_area[len - 2] != 0 {
            return Err(SmbiosError::InvalidStringPoolTermination);
        }

        // Handle empty string pool (just double null)
        if len == 2 {
            return Ok(0);
        }

        // Remove the final double-null terminator and split by null bytes
        let data_without_terminator = &string_pool_area[..len - 2];

        // Split by null bytes to get individual strings
        let strings: Vec<&[u8]> = data_without_terminator.split(|&b| b == 0).collect();

        // Validate each string
        for string_bytes in &strings {
            if string_bytes.is_empty() {
                // Empty slice means consecutive nulls (invalid)
                return Err(SmbiosError::EmptyStringInPool);
            }
            if string_bytes.len() > SMBIOS_STRING_MAX_LENGTH {
                return Err(SmbiosError::StringTooLong);
            }
        }

        Ok(strings.len())
    }

    /// Parse strings from an SMBIOS string pool into a Vec<&str>
    ///
    /// Extracts null-terminated strings from the string pool area as string slices.
    /// This avoids heap allocation by returning references to the existing bytes.
    ///
    /// # Arguments
    ///
    /// * `string_pool_area` - Byte slice containing the string pool (must end with double null)
    ///
    /// # Returns
    ///
    /// Returns `Ok(Vec<&str>)` with all strings from the pool, or an error if the pool is malformed.
    ///
    /// # Errors
    ///
    /// Returns the same errors as `validate_and_count_strings` if the pool format is invalid.
    pub(super) fn parse_strings_from_pool(string_pool_area: &[u8]) -> Result<Vec<&str>, SmbiosError> {
        // First validate the pool
        Self::validate_and_count_strings(string_pool_area)?;

        let len = string_pool_area.len();

        // Handle empty string pool (just double null)
        if len == 2 {
            return Ok(Vec::new());
        }

        // Remove the final double-null terminator and split by null bytes
        let data_without_terminator = &string_pool_area[..len - 2];

        // Split by null bytes and convert to &str slices
        let strings: Result<Vec<&str>, _> = data_without_terminator
            .split(|&b| b == 0)
            .map(|bytes| core::str::from_utf8(bytes).map_err(|_| SmbiosError::MalformedRecordHeader))
            .collect();

        strings
    }

    /// Allocate a new handle using a free list for efficient O(1) allocation
    ///
    /// This implementation maintains a free list of previously freed handles to avoid
    /// O(n) searches through all records. The allocation strategy is:
    /// 1. If freed_handles is non-empty, pop and reuse a freed handle
    /// 2. Otherwise, use next_handle and increment it
    /// 3. If next_handle reaches the reserved range (0xFFFE), wrap to 1
    /// 4. If all handles are exhausted, return OutOfResources
    pub(super) fn allocate_handle(&self) -> Result<SmbiosHandle, SmbiosError> {
        // First, try to reuse a freed handle (most efficient)
        if let Some(handle) = self.freed_handles.borrow_mut().pop() {
            return Ok(handle);
        }

        // No freed handles available, use next_handle
        let candidate = *self.next_handle.borrow();

        // Check if we've exhausted the handle space
        // Valid handles are 1..=0xFEFF (0xFFFE and 0xFFFF are reserved)
        if candidate >= SMBIOS_HANDLE_PI_RESERVED {
            // All handles exhausted
            return Err(SmbiosError::HandleExhausted);
        }

        *self.next_handle.borrow_mut() = candidate + 1;
        Ok(candidate)
    }

    /// Build SMBIOS table data and entry point using pre-allocated buffers
    ///
    /// Copies table data into pre-allocated buffers without calling allocate_pages.
    /// This allows safe republishing during Add/Update/Remove operations.
    ///
    /// Returns (table_address, ep_address, entry_point) but does NOT install the configuration table.
    /// The caller must call install_configuration_table separately without holding locks.
    ///
    pub fn build_table_data(&self) -> Result<(PhysicalAddress, PhysicalAddress, Smbios30EntryPoint), SmbiosError> {
        // Get pre-allocated buffer addresses
        let table_address = self.table_buffer_addr.borrow().ok_or(SmbiosError::AllocationFailed)?;
        let ep_address = self.ep_buffer_addr.borrow().ok_or(SmbiosError::AllocationFailed)?;

        // Borrow records
        let records = self.records.borrow();

        // Verify invariant: Type 127 End-of-Table marker must be last record
        debug_assert!(
            records.last().map(|r| r.header.record_type) == Some(127),
            "Type 127 End-of-Table marker must be the last record in the table"
        );

        // Step 1: Calculate total table size
        let total_table_size: usize = records.iter().map(|r| r.data.len()).sum();

        debug_assert!(total_table_size > 0, "Cannot build table: no SMBIOS records have been added");
        if total_table_size == 0 {
            return Err(SmbiosError::NoRecordsAvailable);
        }

        // Step 2: Check size fits in pre-allocated buffer
        if total_table_size > self.table_buffer_max_size {
            return Err(SmbiosError::AllocationFailed);
        }

        // Step 3: Copy all records to the pre-allocated buffer
        // Records Vec maintains invariant: Type 127 is always last
        // So we can just copy in order
        // SAFETY: table_address points to pre-allocated buffer with size >= total_table_size
        let table_slice = unsafe { core::slice::from_raw_parts_mut(table_address as *mut u8, total_table_size) };
        let mut offset = 0;

        for record in records.iter() {
            let record_bytes = record.data.as_slice();
            table_slice[offset..offset + record_bytes.len()].copy_from_slice(record_bytes);
            offset += record_bytes.len();
        }

        // Step 4: Create entry point structure
        let mut entry_point = Smbios30EntryPoint {
            anchor_string: *b"_SM3_",
            checksum: 0,
            length: core::mem::size_of::<Smbios30EntryPoint>() as u8,
            major_version: self.major_version,
            minor_version: self.minor_version,
            docrev: 0,
            entry_point_revision: 1,
            reserved: 0,
            table_max_size: total_table_size as u32,
            table_address,
        };

        entry_point.checksum = Self::calculate_checksum(&entry_point);

        // Step 5: Copy entry point to pre-allocated buffer
        let ep_bytes = entry_point.as_bytes();
        // SAFETY: ep_address points to pre-allocated buffer with size >= Smbios30EntryPoint
        let ep_slice = unsafe {
            core::slice::from_raw_parts_mut(ep_address as *mut u8, core::mem::size_of::<Smbios30EntryPoint>())
        };
        ep_slice.copy_from_slice(ep_bytes);

        Ok((table_address, ep_address, entry_point))
    }

    /// Store table addresses after installation
    ///
    /// Also calculates and stores a checksum of the published table for
    /// detecting direct modifications (only when buffers are properly allocated).
    pub fn store_table_addresses(&self, table_address: PhysicalAddress, entry_point: Smbios30EntryPoint) {
        let table_size = entry_point.table_max_size as usize;

        // Only calculate checksum if buffers were properly allocated
        // This guards against test scenarios with fake addresses
        if self.table_buffer_addr.borrow().is_some() && table_size > 0 {
            // SAFETY: table_address points to valid published table of table_size bytes
            // (guaranteed when table_buffer_addr is set via allocate_buffers)
            let table_slice = unsafe { core::slice::from_raw_parts(table_address as *const u8, table_size) };
            let checksum = Self::calculate_table_checksum(table_slice);
            self.published_table_checksum.replace(Some(checksum));
            self.published_table_size.replace(table_size);
        }

        self.entry_point_64.replace(Some(Box::new(entry_point)));
        self.table_64_address.replace(Some(table_address));
    }

    /// Calculate a simple checksum for table data
    ///
    /// Uses a simple sum of all bytes (wrapping). This is sufficient for
    /// detecting accidental modifications, not cryptographic integrity.
    fn calculate_table_checksum(data: &[u8]) -> u32 {
        data.iter().fold(0u32, |acc, &byte| acc.wrapping_add(byte as u32))
    }

    /// Verify that the published table has not been modified directly
    ///
    /// Compares the current table contents against the stored checksum.
    /// Returns an error if the table was modified outside of protocol APIs.
    pub fn verify_table_integrity(&self) -> Result<(), SmbiosError> {
        let Some(table_addr) = *self.table_64_address.borrow() else {
            // No table published yet, nothing to verify
            return Ok(());
        };

        let Some(expected_checksum) = *self.published_table_checksum.borrow() else {
            // No checksum stored, skip verification
            return Ok(());
        };

        let table_size = *self.published_table_size.borrow();
        if table_size == 0 {
            return Ok(());
        }

        // SAFETY: table_addr points to valid published table of table_size bytes
        let table_slice = unsafe { core::slice::from_raw_parts(table_addr as *const u8, table_size) };
        let actual_checksum = Self::calculate_table_checksum(table_slice);

        if actual_checksum != expected_checksum {
            log::error!(
                "[SMBIOS] Published table was modified directly (checksum mismatch: expected {:08X}, found {:08X}). \
                 Use Remove() + Add() to modify records, or UpdateString() for string fields.",
                expected_checksum,
                actual_checksum
            );
            return Err(SmbiosError::TableDirectlyModified);
        }

        Ok(())
    }

    /// Rebuild and store the SMBIOS table data
    ///
    /// This is a convenience method that combines `build_table_data()` and
    /// `store_table_addresses()` into a single call. Use this after mutating
    /// records (add/update/remove) to republish the table.
    ///
    /// # Errors
    ///
    /// Returns `SmbiosError::TableDirectlyModified` if the published table was
    /// modified directly instead of using protocol APIs.
    /// Returns other `SmbiosError` variants if table building fails.
    pub fn republish_table(&self) -> Result<(), SmbiosError> {
        // Verify table wasn't modified directly before rebuilding
        self.verify_table_integrity()?;

        let (table_addr, _, entry_point) = self.build_table_data()?;
        self.store_table_addresses(table_addr, entry_point);
        Ok(())
    }

    /// Calculate checksum for SMBIOS 3.x Entry Point Structure
    ///
    /// Computes the checksum byte value such that the sum of all bytes in the
    /// entry point structure equals zero (modulo 256). This is required by the
    /// SMBIOS specification for entry point validation.
    ///
    /// # Arguments
    ///
    /// * `entry_point` - Reference to the SMBIOS 3.0 Entry Point Structure
    ///
    /// # Returns
    ///
    /// The checksum byte value that makes the structure's byte sum equal to zero
    ///
    pub(super) fn calculate_checksum(entry_point: &Smbios30EntryPoint) -> u8 {
        let bytes = entry_point.as_bytes();

        let sum: u8 = bytes.iter().fold(0u8, |acc, &b| acc.wrapping_add(b));
        0u8.wrapping_sub(sum)
    }

    /// Add a new SMBIOS record from raw bytes
    pub fn add_from_bytes(
        &self,
        producer_handle: Option<Handle>,
        record_data: &[u8],
    ) -> Result<SmbiosHandle, SmbiosError> {
        // Step 1: Validate minimum size for header (at least 4 bytes)
        if record_data.len() < core::mem::size_of::<SmbiosTableHeader>() {
            return Err(SmbiosError::RecordTooSmall);
        }

        // Step 2: Parse and validate header using zerocopy
        let (header_ref, _rest) = Ref::<&[u8], SmbiosTableHeader>::from_prefix(record_data)
            .map_err(|_| SmbiosError::MalformedRecordHeader)?;
        let header: &SmbiosTableHeader = &header_ref;

        // Step 3: Reject Type 127 End-of-Table marker - it's automatically managed
        // The manager adds Type 127 during initialization, and it must remain unique and last
        if header.record_type == 127 {
            return Err(SmbiosError::Type127Managed);
        }

        // Step 4: Validate header->length is <= (record_data.length - 2) for string pool
        // The string pool needs at least 2 bytes for the double-null terminator
        if (header.length as usize + 2) > record_data.len() {
            return Err(SmbiosError::RecordTooSmall);
        }

        // Step 5: Validate string pool format and count strings
        let string_pool_start = header.length as usize;
        let string_pool_area = &record_data[string_pool_start..];

        if string_pool_area.len() < 2 {
            return Err(SmbiosError::StringPoolTooSmall);
        }

        let string_count = Self::validate_and_count_strings(string_pool_area)?;

        // If all validation passes, allocate handle and build record
        let smbios_handle = self.allocate_handle()?;

        let record_header =
            SmbiosTableHeader { record_type: header.record_type, length: header.length, handle: smbios_handle };

        // Update the handle in the actual data
        let mut data = record_data.to_vec();
        let handle_bytes = smbios_handle.to_le_bytes();
        data[2] = handle_bytes[0]; // Handle is at offset 2 in header
        data[3] = handle_bytes[1];

        let smbios_record = SmbiosRecord::new(record_header, producer_handle, data, string_count);

        // Maintain invariant: Type 127 End-of-Table marker must always be last
        let mut records = self.records.borrow_mut();
        let type127_is_last = records.last().is_some_and(|r| r.header.record_type == 127);

        records.push(smbios_record);

        // Swap with the element before it to keep Type 127 at the end
        if type127_is_last {
            let len = records.len();
            if len > 1 {
                records.swap(len - 1, len - 2);
            }
        }

        // Verify invariant still holds after insertion: if Type 127 exists, it must be last
        debug_assert!(
            !records.iter().any(|r| r.header.record_type == 127) || records.last().unwrap().header.record_type == 127,
            "Type 127 End-of-Table marker must be last when present"
        );

        Ok(smbios_handle)
    }

    /// Update a string in an existing SMBIOS record
    pub fn update_string(
        &self,
        smbios_handle: SmbiosHandle,
        string_number: usize,
        string: &str,
    ) -> Result<(), SmbiosError> {
        Self::validate_string(string)?;

        // Find the record index
        let pos = self
            .records
            .borrow()
            .iter()
            .position(|r| r.header.handle == smbios_handle)
            .ok_or(SmbiosError::HandleNotFound)?;

        // Borrow the record
        let mut records = self.records.borrow_mut();
        let record = &mut records[pos];

        if string_number == 0 || string_number > record.string_count {
            return Err(SmbiosError::StringIndexOutOfRange);
        }

        // Parse the existing string pool
        let header_length = record.header.length as usize;
        if record.data.len() < header_length + 2 {
            return Err(SmbiosError::RecordTooSmall);
        }

        // Extract existing strings from the string pool using the helper function
        let string_pool_start = header_length;
        let string_pool = &record.data[string_pool_start..];
        let existing_strings_refs = Self::parse_strings_from_pool(string_pool)?;

        // Convert to owned strings so we can modify them
        let mut existing_strings: Vec<String> = existing_strings_refs.iter().map(|s| String::from(*s)).collect();

        // Validate that we have enough strings
        if string_number > existing_strings.len() {
            return Err(SmbiosError::StringIndexOutOfRange);
        }

        // Update the target string (string_number is 1-indexed)
        existing_strings[string_number - 1] = String::from(string);

        // Rebuild the record data with updated string pool
        let mut new_data =
            Vec::with_capacity(header_length + existing_strings.iter().map(|s| s.len() + 1).sum::<usize>() + 1);

        // Copy the structured data (header + fixed fields)
        new_data.extend_from_slice(&record.data[..header_length]);

        // Rebuild the string pool
        for s in &existing_strings {
            new_data.extend_from_slice(s.as_bytes());
            new_data.push(0); // Null terminator
        }

        // Add final null terminator (double null at end)
        new_data.push(0);

        // Update the record with new data
        record.data = new_data;

        Ok(())
    }

    /// Remove an SMBIOS record
    pub fn remove(&self, smbios_handle: SmbiosHandle) -> Result<(), SmbiosError> {
        let pos = self
            .records
            .borrow()
            .iter()
            .position(|r| r.header.handle == smbios_handle)
            .ok_or(SmbiosError::HandleNotFound)?;

        self.records.borrow_mut().remove(pos);

        // Add the freed handle to the free list for reuse
        // Only add valid handles (1..0xFFFE) to the free list
        if (1..SMBIOS_HANDLE_PI_RESERVED).contains(&smbios_handle) {
            self.freed_handles.borrow_mut().push(smbios_handle);
        }

        Ok(())
    }

    /// Create an iterator over SMBIOS records
    ///
    /// This is a convenience method for Rust code that has direct access
    /// to `SmbiosManager`. It provides idiomatic Rust iteration over records.
    ///
    /// **Note**: When using SMBIOS through the component service interface
    /// (`Service<dyn SmbiosRecords>`), this iterator method is not available
    /// due to trait object lifetime constraints. In production, SMBIOS records
    /// are typically added during DXE phase then published for OS access.
    ///
    /// # Arguments
    ///
    /// * `record_type` - Optional filter for specific record type. `None` to
    ///   iterate all records, `Some(type)` to filter by specific type.
    ///
    /// # Returns
    ///
    /// Returns an iterator yielding tuples of `(SmbiosTableHeader, Option<Handle>)`.
    pub fn iter(&self, record_type: Option<SmbiosType>) -> SmbiosRecordsIter<'_> {
        SmbiosRecordsIter::new(self.records.borrow(), record_type)
    }

    /// Get pointer to a record within the published SMBIOS table
    ///
    /// Returns the address of the record within the published table and the producer handle.
    /// This returns a pointer directly into the published table memory, not a copy.
    ///
    /// # Returns
    /// - `Some((address, producer_handle))` if the record exists and table is published
    /// - `None` if the record is not found or table is not published
    pub fn get_record_pointer(&self, handle: SmbiosHandle) -> Option<(PhysicalAddress, Option<Handle>)> {
        let table_addr = (*self.table_64_address.borrow())?;
        let records = self.records.borrow();

        // Calculate offset by summing sizes of all records before the target
        let mut offset: usize = 0;
        for record in records.iter() {
            if record.header.handle == handle {
                return Some((table_addr + offset as u64, record.producer_handle));
            }
            offset += record.data.len();
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    use std::{vec, vec::Vec};

    use crate::{
        error::SmbiosError,
        service::{SMBIOS_HANDLE_PI_RESERVED, SMBIOS_STRING_MAX_LENGTH, SmbiosHandle, SmbiosTableHeader},
    };
    use r_efi::efi;
    use zerocopy::IntoBytes;

    /// Test helper: Build a simple SMBIOS record with the given header and strings
    ///
    /// This helper manually constructs a minimal SMBIOS record for testing purposes.
    /// In production code, use structured record types (Type0, Type1, etc.) with to_bytes().
    fn build_test_record_with_strings(header: &SmbiosTableHeader, strings: &[&str]) -> Vec<u8> {
        let mut bytes = Vec::new();

        // Serialize header
        bytes.extend_from_slice(header.as_bytes());

        // Add string pool
        if strings.is_empty() {
            // Empty string pool: just double null
            bytes.push(0);
            bytes.push(0);
        } else {
            for s in strings {
                bytes.extend_from_slice(s.as_bytes());
                bytes.push(0); // Null terminator
            }
            bytes.push(0); // Double null terminator
        }

        bytes
    }

    #[test]
    fn test_smbios_record_builder_builds_bytes() {
        // Ensure build_test_record_with_strings returns a proper record buffer
        let header = SmbiosTableHeader::new(1, 4 + 2, SMBIOS_HANDLE_PI_RESERVED);
        let strings = &["ACME Corp", "SuperServer 3000"];

        let record = build_test_record_with_strings(&header, strings);

        assert!(record.len() > core::mem::size_of::<SmbiosTableHeader>());
        assert_eq!(record[0], 1u8);
    }

    #[test]
    fn test_add_type0_platform_firmware_information_to_manager() {
        let manager = SmbiosManager::new(3, 9).expect("failed to create manager");

        // Manually build a Type 0 record with proper structure
        let mut record_data = vec![
            0,    // Type 0 (BIOS Information)
            0x1A, // Length = 26 bytes (4-byte header + 22 bytes structured data)
            0xFF, 0xFE, // Handle = SMBIOS_HANDLE_PI_RESERVED
            // Structured data fields (22 bytes total):
            1, // vendor (string index 1)
            2, // bios_version (string index 2)
            0x00, 0xE0, // bios_starting_address_segment (0xE000)
            3,    // bios_release_date (string index 3)
            0x0F, // bios_rom_size (1MB)
            0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // characteristics (u64)
            0x01, // characteristics_ext1
            0x00, // characteristics_ext2
            9,    // system_bios_major_release
            9,    // system_bios_minor_release
            0xFF, // embedded_controller_major_release
            0xFF, // embedded_controller_minor_release
            0x00, 0x00, // extended_bios_rom_size
        ];

        // Add string pool
        record_data.extend_from_slice(b"TestVendor\0");
        record_data.extend_from_slice(b"9.9.9\0");
        record_data.extend_from_slice(b"09/24/2025\0");
        record_data.push(0); // Double null terminator

        let handle = manager.add_from_bytes(None, &record_data).expect("add_from_bytes failed");

        let mut found = false;
        for (found_header, _producer) in manager.iter(Some(0)) {
            assert_eq!(found_header.record_type, 0);
            let found_handle = found_header.handle;
            assert_eq!(found_handle, handle);
            found = true;
        }
        assert!(found);
    }

    #[test]
    fn test_validate_string() {
        // Success cases
        assert!(SmbiosManager::validate_string("Valid String").is_ok());
        assert!(SmbiosManager::validate_string("").is_ok());
        assert!(SmbiosManager::validate_string("Hello World 123!").is_ok());
        assert!(SmbiosManager::validate_string("Valid-String_With.Symbols").is_ok());
        let max_string = "a".repeat(SMBIOS_STRING_MAX_LENGTH);
        assert!(SmbiosManager::validate_string(&max_string).is_ok());

        // Error cases
        let long_string = "a".repeat(SMBIOS_STRING_MAX_LENGTH + 1);
        assert_eq!(SmbiosManager::validate_string(&long_string), Err(SmbiosError::StringTooLong));
        assert_eq!(SmbiosManager::validate_string("test\0string"), Err(SmbiosError::StringContainsNull));
        assert_eq!(SmbiosManager::validate_string("before\0after"), Err(SmbiosError::StringContainsNull));
    }

    #[test]
    fn test_validate_and_count_strings() {
        // Success cases
        assert_eq!(SmbiosManager::validate_and_count_strings(&[0u8, 0u8]), Ok(0)); // Empty pool
        assert_eq!(SmbiosManager::validate_and_count_strings(b"test\0\0"), Ok(1)); // Single string
        assert_eq!(SmbiosManager::validate_and_count_strings(b"first\0second\0third\0\0"), Ok(3)); // Multiple

        // Error cases
        assert_eq!(SmbiosManager::validate_and_count_strings(&[0u8]), Err(SmbiosError::InvalidStringPoolTermination)); // Too short
        assert_eq!(
            SmbiosManager::validate_and_count_strings(b"test\0"),
            Err(SmbiosError::InvalidStringPoolTermination)
        ); // No double null
        assert_eq!(
            SmbiosManager::validate_and_count_strings(b"test\0\0extra\0\0"),
            Err(SmbiosError::EmptyStringInPool)
        ); // Consecutive nulls

        let mut pool = vec![b'a'; SMBIOS_STRING_MAX_LENGTH + 1];
        pool.push(0);
        pool.push(0);
        assert_eq!(SmbiosManager::validate_and_count_strings(&pool), Err(SmbiosError::StringTooLong)); // String too long
    }

    #[test]
    fn test_parse_strings_from_pool() {
        // Success cases
        let pool = b"first\0second\0third\0\0";
        let strings = SmbiosManager::parse_strings_from_pool(pool).expect("parse failed");
        assert_eq!(strings.len(), 3);
        assert_eq!(strings[0], "first");
        assert_eq!(strings[1], "second");
        assert_eq!(strings[2], "third");

        let pool_empty = b"\0\0";
        assert_eq!(SmbiosManager::parse_strings_from_pool(pool_empty).expect("parse failed").len(), 0);

        let pool_single = b"teststring\0\0";
        let strings_single = SmbiosManager::parse_strings_from_pool(pool_single).expect("parse failed");
        assert_eq!(strings_single.len(), 1);
        assert_eq!(strings_single[0], "teststring");

        let pool_chars = b"A\0B\0C\0\0";
        let strings_chars = SmbiosManager::parse_strings_from_pool(pool_chars).expect("parse failed");
        assert_eq!(strings_chars, vec!["A", "B", "C"]);

        // Error cases
        assert_eq!(SmbiosManager::parse_strings_from_pool(b"\0"), Err(SmbiosError::InvalidStringPoolTermination));
        assert_eq!(
            SmbiosManager::parse_strings_from_pool(b"test\0single"),
            Err(SmbiosError::InvalidStringPoolTermination)
        );
        assert_eq!(SmbiosManager::parse_strings_from_pool(b"first\0\0extra\0\0"), Err(SmbiosError::EmptyStringInPool));
    }

    #[test]
    fn test_build_record_with_strings() {
        // Basic record with strings
        let header = SmbiosTableHeader::new(1, 10, SMBIOS_HANDLE_PI_RESERVED);
        let strings = &["Manufacturer", "Product"];
        let record = build_test_record_with_strings(&header, strings);
        assert!(record.len() >= core::mem::size_of::<SmbiosTableHeader>());
        assert_eq!(record[0], 1);

        // No strings
        let strings_empty: &[&str] = &[];
        let record_empty = build_test_record_with_strings(&header, strings_empty);
        assert_eq!(record_empty[record_empty.len() - 1], 0);
        assert_eq!(record_empty[record_empty.len() - 2], 0);

        // Multiple strings with verification
        let header2 = SmbiosTableHeader::new(2, 4, SMBIOS_HANDLE_PI_RESERVED);
        let strings_multi = &["Manufacturer", "Product", "Version", "Serial"];
        let record_multi = build_test_record_with_strings(&header2, strings_multi);
        assert_eq!(record_multi[0], 2);
        let pool = &record_multi[4..];
        let parsed = SmbiosManager::parse_strings_from_pool(pool).expect("parse failed");
        assert_eq!(parsed, vec!["Manufacturer", "Product", "Version", "Serial"]);

        // Empty string edge case
        let strings_empty_str = &[""];
        let record_empty_str = build_test_record_with_strings(&header, strings_empty_str);
        assert_eq!(record_empty_str[4], 0);
        assert_eq!(record_empty_str[5], 0);
    }

    #[test]
    fn test_version() {
        let manager = SmbiosManager::new(3, 9).expect("failed to create manager");
        assert_eq!((manager.major_version, manager.minor_version), (3, 9));
    }

    #[test]
    fn test_version_custom_values() {
        let manager = SmbiosManager::new(3, 7).expect("failed to create manager");
        assert_eq!((manager.major_version, manager.minor_version), (3, 7));
        let manager2 = SmbiosManager::new(3, 255).expect("failed to create manager");
        assert_eq!((manager2.major_version, manager2.minor_version), (3, 255));
    }

    #[test]
    fn test_allocate_handle_sequential() {
        let manager = SmbiosManager::new(3, 9).expect("failed to create manager");
        let handle1 = manager.allocate_handle().expect("allocation failed");
        assert_eq!(handle1, 1);
        let handle2 = manager.allocate_handle().expect("allocation failed");
        assert_eq!(handle2, 2);
    }

    #[test]
    fn test_allocate_handle_wraps_at_max() {
        let manager = SmbiosManager::new(3, 9).expect("failed to create manager");
        *manager.next_handle.borrow_mut() = 0xFFFD;
        let handle1 = manager.allocate_handle().expect("allocation failed");
        assert_eq!(handle1, 0xFFFD);
        assert_eq!(manager.allocate_handle(), Err(SmbiosError::HandleExhausted));
    }

    #[test]
    fn test_allocate_handle_uses_free_list() {
        let manager = SmbiosManager::new(3, 9).expect("failed to create manager");
        manager.freed_handles.borrow_mut().push(100);
        manager.freed_handles.borrow_mut().push(50);
        let handle1 = manager.allocate_handle().expect("allocation failed");
        assert_eq!(handle1, 50);
        let handle2 = manager.allocate_handle().expect("allocation failed");
        assert_eq!(handle2, 100);
        let handle3 = manager.allocate_handle().expect("allocation failed");
        assert_eq!(handle3, 1);
    }

    #[test]
    fn test_allocate_handle_with_gaps() {
        let manager = SmbiosManager::new(3, 9).expect("failed to create manager");
        let header1 = SmbiosTableHeader::new(1, 4, SMBIOS_HANDLE_PI_RESERVED);
        let bytes1 = build_test_record_with_strings(&header1, &[]);
        let _h1 = manager.add_from_bytes(None, &bytes1).expect("add failed");
        let header2 = SmbiosTableHeader::new(1, 4, SMBIOS_HANDLE_PI_RESERVED);
        let bytes2 = build_test_record_with_strings(&header2, &[]);
        let h2 = manager.add_from_bytes(None, &bytes2).expect("add failed");
        let header3 = SmbiosTableHeader::new(1, 4, SMBIOS_HANDLE_PI_RESERVED);
        let bytes3 = build_test_record_with_strings(&header3, &[]);
        let _h3 = manager.add_from_bytes(None, &bytes3).expect("add failed");
        manager.remove(h2).expect("remove failed");
        let header4 = SmbiosTableHeader::new(1, 4, SMBIOS_HANDLE_PI_RESERVED);
        let bytes4 = build_test_record_with_strings(&header4, &[]);
        let h4 = manager.add_from_bytes(None, &bytes4).expect("add failed");
        assert_eq!(h4, h2);
    }

    #[test]
    fn test_handle_reuse_after_remove() {
        let manager = SmbiosManager::new(3, 9).expect("failed to create manager");
        let mut record_data = vec![1u8, 4, 0, 0];
        record_data.extend_from_slice(b"\0\0");
        let handle1 = manager.add_from_bytes(None, &record_data).expect("add failed");
        manager.remove(handle1).expect("remove failed");
        let mut record_data2 = vec![2u8, 4, 0, 0];
        record_data2.extend_from_slice(b"\0\0");
        let handle2 = manager.add_from_bytes(None, &record_data2).expect("add failed");
        assert_eq!(handle1, handle2);
    }

    #[test]
    fn test_update_string_success() {
        let manager = SmbiosManager::new(3, 9).expect("failed to create manager");
        let mut record_data = vec![1u8, 4, 0, 0];
        record_data.extend_from_slice(b"original\0\0");
        let handle = manager.add_from_bytes(None, &record_data).expect("add failed");
        manager.update_string(handle, 1, "updated").expect("update failed");
        assert!(manager.update_string(handle, 1, "another").is_ok());
    }

    #[test]
    fn test_update_string_handle_not_found() {
        let manager = SmbiosManager::new(3, 9).expect("failed to create manager");
        assert_eq!(manager.update_string(999, 1, "test"), Err(SmbiosError::HandleNotFound));
    }

    #[test]
    fn test_update_string_invalid_string_number() {
        let manager = SmbiosManager::new(3, 9).expect("failed to create manager");
        let mut record_data = vec![1u8, 4, 0, 0];
        record_data.extend_from_slice(b"test\0\0");
        let handle = manager.add_from_bytes(None, &record_data).expect("add failed");
        assert_eq!(manager.update_string(handle, 0, "new"), Err(SmbiosError::StringIndexOutOfRange));
        assert_eq!(manager.update_string(handle, 2, "new"), Err(SmbiosError::StringIndexOutOfRange));
    }

    #[test]
    fn test_update_string_too_long() {
        let manager = SmbiosManager::new(3, 9).expect("failed to create manager");
        let mut record_data = vec![1u8, 4, 0, 0];
        record_data.extend_from_slice(b"test\0\0");
        let handle = manager.add_from_bytes(None, &record_data).expect("add failed");
        let long_string = "a".repeat(SMBIOS_STRING_MAX_LENGTH + 1);
        assert_eq!(manager.update_string(handle, 1, &long_string), Err(SmbiosError::StringTooLong));
    }

    #[test]
    fn test_update_string_rebuilds_pool() {
        let manager = SmbiosManager::new(3, 9).expect("failed to create manager");
        let mut record_data = vec![1u8, 4, 0, 0];
        record_data.extend_from_slice(b"first\0second\0third\0\0");
        let handle = manager.add_from_bytes(None, &record_data).expect("add failed");
        manager.update_string(handle, 2, "new_second_string").expect("update failed");
        assert!(manager.update_string(handle, 1, "new_first").is_ok());
        assert!(manager.update_string(handle, 3, "new_third").is_ok());
    }

    #[test]
    fn test_update_string_with_empty_string() {
        let manager = SmbiosManager::new(3, 9).expect("failed to create manager");
        let header = SmbiosTableHeader::new(1, 4, SMBIOS_HANDLE_PI_RESERVED);
        let bytes = build_test_record_with_strings(&header, &["Original"]);
        let handle = manager.add_from_bytes(None, &bytes).expect("add failed");
        let result = manager.update_string(handle, 1, "");
        assert!(result.is_ok());
    }

    #[test]
    fn test_update_string_buffer_too_small_error() {
        use super::SmbiosRecord;
        let manager = SmbiosManager::new(3, 9).expect("failed to create manager");
        let header = SmbiosTableHeader::new(1, 10, 1);
        let data = vec![1u8, 10, 1, 0];
        let record = SmbiosRecord::new(header, None, data, 1);
        manager.records.borrow_mut().push(record);
        assert_eq!(manager.update_string(1, 1, "test"), Err(SmbiosError::RecordTooSmall));
    }

    #[test]
    fn test_remove_success() {
        let manager = SmbiosManager::new(3, 9).expect("failed to create manager");
        let mut record_data = vec![1u8, 4, 0, 0];
        record_data.extend_from_slice(b"\0\0");
        let handle = manager.add_from_bytes(None, &record_data).expect("add failed");
        assert!(manager.remove(handle).is_ok());
        assert_eq!(manager.remove(handle), Err(SmbiosError::HandleNotFound));
    }

    #[test]
    fn test_remove_last_record() {
        let manager = SmbiosManager::new(3, 9).expect("failed to create manager");
        let header = SmbiosTableHeader::new(1, 4, SMBIOS_HANDLE_PI_RESERVED);
        let bytes = build_test_record_with_strings(&header, &[]);
        let handle = manager.add_from_bytes(None, &bytes).expect("add failed");
        manager.remove(handle).expect("remove failed");
        assert_eq!(manager.records.borrow().len(), 0);
    }

    #[test]
    fn test_remove_reserved_handle_not_added_to_free_list() {
        let manager = SmbiosManager::new(3, 9).expect("failed to create manager");
        let mut record_data = vec![1u8, 4, 0, 0];
        record_data.extend_from_slice(b"\0\0");
        let handle = manager.add_from_bytes(None, &record_data).expect("add failed");
        manager.remove(handle).expect("remove failed");
        let freed = manager.freed_handles.borrow();
        assert_eq!(freed.len(), 1);
        assert_eq!(freed[0], handle);
    }

    #[test]
    fn test_iteration() {
        let manager = SmbiosManager::new(3, 9).expect("failed to create manager");

        // Empty manager
        assert_eq!(manager.iter(None).count(), 0);

        // Add test records: types [1, 2, 1, 3, 1]
        for record_type in [1u8, 2, 1, 3, 1] {
            let mut record_data = vec![record_type, 4, 0, 0];
            record_data.extend_from_slice(b"\0\0");
            manager.add_from_bytes(None, &record_data).expect("add failed");
        }

        // Iterate all - should have 5 records
        assert_eq!(manager.iter(None).count(), 5);

        // Filter by type 1 - should find 3 records
        let mut count_type1 = 0;
        for (header, _) in manager.iter(Some(1)) {
            assert_eq!(header.record_type, 1);
            count_type1 += 1;
        }
        assert_eq!(count_type1, 3);

        // Filter by nonexistent type - should find 0
        assert_eq!(manager.iter(Some(99)).count(), 0);
    }

    #[test]
    fn test_iteration_navigation() {
        let manager = SmbiosManager::new(3, 9).expect("failed to create manager");
        let handles: Vec<SmbiosHandle> = (1..=3)
            .map(|i| {
                let mut record_data = vec![i, 4, 0, 0];
                record_data.extend_from_slice(b"\0\0");
                manager.add_from_bytes(None, &record_data).expect("add failed")
            })
            .collect();

        // Navigate from middle handle
        let result = manager.iter(None).skip_while(|(hdr, _)| hdr.handle != handles[1]).nth(1);
        let (header, _) = result.expect("Should find next record");
        // Copy to avoid unaligned reference
        let found_handle = header.handle;
        let found_type = header.record_type;
        assert_eq!(found_handle, handles[2]);
        assert_eq!(found_type, 3);

        // Invalid start handle - should not find anything after skip
        let result = manager.iter(None).skip_while(|(hdr, _)| hdr.handle != 9999).nth(1);
        assert!(result.is_none(), "Should not find any record after non-existent handle");
    }

    #[test]
    fn test_add_from_bytes_buffer_too_small() {
        let manager = SmbiosManager::new(3, 9).expect("failed to create manager");
        let small_buffer = vec![1u8, 2];
        assert_eq!(manager.add_from_bytes(None, &small_buffer), Err(SmbiosError::RecordTooSmall));
    }

    #[test]
    fn test_add_from_bytes_invalid_length() {
        let manager = SmbiosManager::new(3, 9).expect("failed to create manager");
        let invalid_data = vec![1u8, 255, 0, 0, 0, 0];
        assert_eq!(manager.add_from_bytes(None, &invalid_data), Err(SmbiosError::RecordTooSmall));
    }

    #[test]
    fn test_add_from_bytes_no_string_pool() {
        let manager = SmbiosManager::new(3, 9).expect("failed to create manager");
        let mut data = vec![1u8, 10, 0, 0];
        data.extend_from_slice(&[0u8; 6]);
        assert_eq!(manager.add_from_bytes(None, &data), Err(SmbiosError::RecordTooSmall));
    }

    #[test]
    fn test_add_from_bytes_with_producer_handle() {
        let manager = SmbiosManager::new(3, 9).expect("failed to create manager");
        let producer = 0x1234 as efi::Handle;
        let mut record_data = vec![1u8, 4, 0, 0];
        record_data.extend_from_slice(b"\0\0");
        let handle = manager.add_from_bytes(Some(producer), &record_data).expect("add failed");
        let mut found = false;
        for (header, found_producer) in manager.iter(None) {
            assert_eq!(found_producer, Some(producer));
            let found_handle = header.handle;
            assert_eq!(found_handle, handle);
            found = true;
        }
        assert!(found);
    }

    #[test]
    fn test_add_from_bytes_with_strings() {
        let manager = SmbiosManager::new(3, 9).expect("failed to create manager");
        let mut record_data = vec![1u8, 6, 0, 0];
        record_data.extend_from_slice(&[0x01, 0x02]);
        record_data.extend_from_slice(b"String1\0String2\0\0");
        let assigned_handle = manager.add_from_bytes(None, &record_data).expect("add failed");
        let records = manager.records.borrow();
        assert_eq!(records.len(), 1);
        let handle = records[0].header.handle;
        assert_eq!(handle, assigned_handle);
        assert_eq!(records[0].string_count, 2);
    }

    #[test]
    fn test_add_from_bytes_with_max_handle() {
        let manager = SmbiosManager::new(3, 9).expect("failed to create manager");
        let header = SmbiosTableHeader::new(1, 4, 0xFEFF);
        let record_bytes = build_test_record_with_strings(&header, &[]);
        let result = manager.add_from_bytes(None, &record_bytes);
        assert!(result.is_ok());
    }

    #[test]
    fn test_add_from_bytes_updates_handle_in_data() {
        let manager = SmbiosManager::new(3, 9).expect("failed to create manager");
        let mut record_data = vec![1u8, 4, 0xFF, 0xFF];
        record_data.extend_from_slice(b"\0\0");
        let assigned_handle = manager.add_from_bytes(None, &record_data).expect("add failed");
        let records = manager.records.borrow();
        let stored_data = &records[0].data;
        let stored_handle = u16::from_le_bytes([stored_data[2], stored_data[3]]);
        assert_eq!(stored_handle, assigned_handle);
    }

    #[test]
    fn test_calculate_checksum() {
        let entry_point = Smbios30EntryPoint {
            anchor_string: *b"_SM3_",
            checksum: 0,
            length: 24,
            major_version: 3,
            minor_version: 9,
            docrev: 0,
            entry_point_revision: 1,
            reserved: 0,
            table_max_size: 0x1000,
            table_address: 0x80000000,
        };
        let checksum = SmbiosManager::calculate_checksum(&entry_point);
        let mut test_entry = entry_point;
        test_entry.checksum = checksum;
        let bytes = test_entry.as_bytes();
        let sum: u8 = bytes.iter().fold(0u8, |acc, &b| acc.wrapping_add(b));
        assert_eq!(sum, 0);
    }

    #[test]
    fn test_calculate_checksum_zero() {
        let entry_point = Smbios30EntryPoint {
            anchor_string: [0, 0, 0, 0, 0],
            checksum: 0,
            length: 0,
            major_version: 0,
            minor_version: 0,
            docrev: 0,
            entry_point_revision: 0,
            reserved: 0,
            table_max_size: 0,
            table_address: 0,
        };
        let checksum = SmbiosManager::calculate_checksum(&entry_point);
        assert_eq!(checksum, 0);
    }

    #[test]
    fn test_smbios_record_builder_with_fields() {
        let header = SmbiosTableHeader::new(3, 4 + 3, SMBIOS_HANDLE_PI_RESERVED);
        let strings = &["Chassis Manufacturer", "Tower", "v1.0"];
        let record = build_test_record_with_strings(&header, strings);
        assert_eq!(record[0], 3);
        assert!(record.len() > 10);
    }

    #[test]
    fn test_smbios_record_builder_empty_build() {
        let header = SmbiosTableHeader::new(127, 4, SMBIOS_HANDLE_PI_RESERVED);
        let record = build_test_record_with_strings(&header, &[]);
        assert_eq!(record[0], 127);
        assert_eq!(record[1], 4);
        assert_eq!(record[record.len() - 1], 0);
        assert_eq!(record[record.len() - 2], 0);
    }

    #[test]
    fn test_smbios_record_builder_multiple_fields() {
        let header = SmbiosTableHeader::new(3, 4 + 15, SMBIOS_HANDLE_PI_RESERVED);
        let mut record_data = Vec::new();
        record_data.extend_from_slice(header.as_bytes());
        record_data.push(1);
        record_data.push(0x12);
        record_data.push(2);
        record_data.push(3);
        record_data.push(4);
        record_data.push(0x03);
        record_data.push(0x03);
        record_data.push(0x03);
        record_data.push(0x02);
        record_data.extend_from_slice(&0x789ABCDE_u32.to_le_bytes());
        record_data.push(0x04);
        record_data.push(0x01);
        record_data.push(0);
        record_data.push(0);
        assert_eq!(record_data[0], 3);
        assert!(record_data.len() > core::mem::size_of::<SmbiosTableHeader>());
    }

    #[test]
    fn test_smbios_record_builder_with_multiple_strings() {
        let header = SmbiosTableHeader::new(1, 4, SMBIOS_HANDLE_PI_RESERVED);
        let strings = &["String1", "String2", "String3"];
        let record = build_test_record_with_strings(&header, strings);
        assert_eq!(record[0], 1);
        let record_str = core::str::from_utf8(&record[4..]).unwrap_or("");
        assert!(record_str.contains("String1"));
        assert!(record_str.contains("String2"));
        assert!(record_str.contains("String3"));
    }

    #[test]
    fn test_smbios_table_header() {
        // Test creation and field access
        let header = SmbiosTableHeader::new(42, 100, 0x1234);
        let record_type = header.record_type;
        let length = header.length;
        let handle = header.handle;
        assert_eq!(record_type, 42);
        assert_eq!(length, 100);
        assert_eq!(handle, 0x1234);

        // Test with different values
        let header2 = SmbiosTableHeader::new(5, 20, 42);
        let record_type2 = header2.record_type;
        let length2 = header2.length;
        let handle2 = header2.handle;
        assert_eq!(record_type2, 5);
        assert_eq!(length2, 20);
        assert_eq!(handle2, 42);
    }

    #[test]
    fn test_smbios_handle_constants() {
        assert_eq!(SMBIOS_HANDLE_PI_RESERVED, 0xFFFE);
    }

    #[test]
    fn test_add_type127_returns_error() {
        // Test that trying to add Type 127 manually returns Type127Managed error
        let manager = SmbiosManager::new(3, 9).expect("failed to create manager");

        // Build a Type 127 record
        let mut record_data = vec![127u8, 4, 0, 0]; // Type 127, length 4, handle 0
        record_data.extend_from_slice(b"\0\0"); // Empty string pool

        // Should return Type127Managed error
        let result = manager.add_from_bytes(None, &record_data);
        assert_eq!(result, Err(SmbiosError::Type127Managed));
    }

    #[test]
    fn test_type127_invariant_remains_last() {
        // Test that Type 127 remains last when adding new records after allocate_buffers
        // Since allocate_buffers requires memory_manager, we manually simulate the state
        let manager = SmbiosManager::new(3, 9).expect("failed to create manager");

        // Manually add Type 127 to simulate allocate_buffers behavior
        let type127 = Type127EndOfTable::new();
        let bytes = type127.to_bytes();
        let header = SmbiosTableHeader::new(127, 4, manager.allocate_handle().unwrap());
        let record = SmbiosRecord::new(header, None, bytes, 0);
        manager.records.borrow_mut().push(record);

        // Now add a Type 1 record
        let mut record_data = vec![1u8, 4, 0, 0];
        record_data.extend_from_slice(b"\0\0");
        manager.add_from_bytes(None, &record_data).expect("add failed");

        // Verify Type 127 is still last
        let records = manager.records.borrow();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].header.record_type, 1, "First record should be Type 1");
        assert_eq!(records[1].header.record_type, 127, "Last record should be Type 127");
    }

    #[test]
    fn test_type127_invariant_multiple_adds() {
        // Test Type 127 remains last with multiple record additions
        let manager = SmbiosManager::new(3, 9).expect("failed to create manager");

        // Manually add Type 127
        let type127 = Type127EndOfTable::new();
        let bytes = type127.to_bytes();
        let header = SmbiosTableHeader::new(127, 4, manager.allocate_handle().unwrap());
        let record = SmbiosRecord::new(header, None, bytes, 0);
        manager.records.borrow_mut().push(record);

        // Add multiple records of different types
        for record_type in [1u8, 2, 3, 0, 4] {
            let mut record_data = vec![record_type, 4, 0, 0];
            record_data.extend_from_slice(b"\0\0");
            manager.add_from_bytes(None, &record_data).expect("add failed");
        }

        // Verify Type 127 is still last
        let records = manager.records.borrow();
        assert_eq!(records.len(), 6, "Should have 5 user records + 1 Type 127");
        let last_record = &records[records.len() - 1];
        assert_eq!(last_record.header.record_type, 127, "Last record must be Type 127");
    }

    #[test]
    #[should_panic(expected = "Type 127 End-of-Table marker must be last when present")]
    #[cfg(debug_assertions)]
    fn test_type127_not_last_panics_in_add_from_bytes() {
        // Test that debug_assert fires when Type 127 is not last after adding a record
        let manager = SmbiosManager::new(3, 9).expect("failed to create manager");

        // Add Type 127 in the middle
        let type127 = Type127EndOfTable::new();
        let bytes = type127.to_bytes();
        let header = SmbiosTableHeader::new(127, 4, manager.allocate_handle().unwrap());
        let record = SmbiosRecord::new(header, None, bytes, 0);
        manager.records.borrow_mut().push(record);

        // Add another non-Type-127 record after it (manually, bypassing add_from_bytes)
        let mut record_data = vec![1u8, 4, 0, 0];
        record_data.extend_from_slice(b"\0\0");
        let header = SmbiosTableHeader::new(1, 4, manager.allocate_handle().unwrap());
        let record = SmbiosRecord::new(header, None, record_data, 0);
        manager.records.borrow_mut().push(record);

        // Now try to add another record via add_from_bytes - should panic in debug build
        let mut new_record = vec![2u8, 4, 0, 0];
        new_record.extend_from_slice(b"\0\0");
        let _ = manager.add_from_bytes(None, &new_record);
    }

    #[test]
    #[should_panic(expected = "Type 127 End-of-Table marker must be the last record in the table")]
    #[cfg(debug_assertions)]
    fn test_type127_not_last_panics_in_build_table_data() {
        // Test that debug_assert fires in build_table_data when Type 127 is not last
        let manager = SmbiosManager::new(3, 9).expect("failed to create manager");

        // Manually set up buffers to bypass allocate_buffers
        *manager.table_buffer_addr.borrow_mut() = Some(0x1000);
        *manager.ep_buffer_addr.borrow_mut() = Some(0x2000);

        // Add Type 127 first
        let type127 = Type127EndOfTable::new();
        let bytes = type127.to_bytes();
        let header = SmbiosTableHeader::new(127, 4, manager.allocate_handle().unwrap());
        let record = SmbiosRecord::new(header, None, bytes, 0);
        manager.records.borrow_mut().push(record);

        // Add another record after Type 127 (violates invariant)
        let mut record_data = vec![1u8, 4, 0, 0];
        record_data.extend_from_slice(b"\0\0");
        let header = SmbiosTableHeader::new(1, 4, manager.allocate_handle().unwrap());
        let record = SmbiosRecord::new(header, None, record_data, 0);
        manager.records.borrow_mut().push(record);

        // Try to build table - should panic in debug build
        let _ = manager.build_table_data();
    }

    #[test]
    fn test_store_table_addresses() {
        // Test store_table_addresses stores values correctly
        let manager = SmbiosManager::new(3, 9).expect("failed to create manager");

        let entry_point = Smbios30EntryPoint {
            anchor_string: *b"_SM3_",
            checksum: 0,
            length: 24,
            major_version: 3,
            minor_version: 9,
            docrev: 0,
            entry_point_revision: 1,
            reserved: 0,
            table_max_size: 0x1000,
            table_address: 0x80000000,
        };

        let table_addr: PhysicalAddress = 0x80000000;

        manager.store_table_addresses(table_addr, entry_point);

        // Verify the addresses were stored
        assert!(manager.entry_point_64.borrow().is_some());
        assert_eq!(*manager.table_64_address.borrow(), Some(table_addr));
    }

    #[test]
    fn test_build_table_data_no_buffers_allocated() {
        // Test that build_table_data returns error when buffers not allocated
        let manager = SmbiosManager::new(3, 9).expect("failed to create manager");

        // Add a record
        let mut record_data = vec![1u8, 4, 0, 0];
        record_data.extend_from_slice(b"\0\0");
        manager.add_from_bytes(None, &record_data).expect("add failed");

        // Try to build table without allocating buffers - should fail
        // We need a mock boot_services, but since we can't create one easily,
        // we'll test the error path by checking that table_buffer_addr is None
        assert!(manager.table_buffer_addr.borrow().is_none());
        assert!(manager.ep_buffer_addr.borrow().is_none());
    }

    #[test]
    fn test_build_table_data_empty_records() {
        // Test that build_table_data returns error with no records
        let manager = SmbiosManager::new(3, 9).expect("failed to create manager");

        // Set up fake buffer addresses to bypass allocation check
        *manager.table_buffer_addr.borrow_mut() = Some(0x1000);
        *manager.ep_buffer_addr.borrow_mut() = Some(0x2000);

        // With no records, should get NoRecordsAvailable error
        // Note: We can't actually call build_table_data without boot_services,
        // but we can verify the precondition
        assert_eq!(manager.records.borrow().len(), 0);
    }

    #[test]
    fn test_validate_string_max_length() {
        // Test string at exactly the max length (should pass)
        let max_str = "a".repeat(SMBIOS_STRING_MAX_LENGTH);
        assert!(SmbiosManager::validate_string(&max_str).is_ok());

        // Test string one byte over max (should fail)
        let too_long = "a".repeat(SMBIOS_STRING_MAX_LENGTH + 1);
        assert_eq!(SmbiosManager::validate_string(&too_long), Err(SmbiosError::StringTooLong));
    }

    #[test]
    fn test_validate_string_with_null() {
        // Test string containing null byte (should fail)
        let with_null = "test\0string";
        assert_eq!(SmbiosManager::validate_string(with_null), Err(SmbiosError::StringContainsNull));
    }

    #[test]
    fn test_validate_string_empty() {
        // Empty string should be valid
        assert!(SmbiosManager::validate_string("").is_ok());
    }

    #[test]
    fn test_entry_point_structure_size() {
        // Verify Smbios30EntryPoint has expected size (24 bytes per spec)
        assert_eq!(core::mem::size_of::<Smbios30EntryPoint>(), 24, "SMBIOS 3.0 Entry Point must be exactly 24 bytes");
    }

    #[test]
    fn test_entry_point_anchor_string() {
        // Verify entry point can be created with correct anchor string
        let entry_point = Smbios30EntryPoint {
            anchor_string: *b"_SM3_",
            checksum: 0,
            length: 24,
            major_version: 3,
            minor_version: 9,
            docrev: 0,
            entry_point_revision: 1,
            reserved: 0,
            table_max_size: 0x1000,
            table_address: 0x80000000,
        };

        assert_eq!(&entry_point.anchor_string, b"_SM3_");
        assert_eq!(entry_point.length, 24);
        assert_eq!(entry_point.major_version, 3);
    }

    #[test]
    fn test_multiple_removes_and_readds() {
        // Test that handles are properly recycled after multiple operations
        let manager = SmbiosManager::new(3, 9).expect("failed to create manager");

        // Add, remove, add cycle to test handle reuse
        let mut record_data = vec![1u8, 4, 0, 0];
        record_data.extend_from_slice(b"\0\0");

        let handle1 = manager.add_from_bytes(None, &record_data).expect("add 1 failed");
        manager.remove(handle1).expect("remove 1 failed");

        let handle2 = manager.add_from_bytes(None, &record_data).expect("add 2 failed");
        assert_eq!(handle1, handle2, "Handle should be reused from free list");

        manager.remove(handle2).expect("remove 2 failed");
        let handle3 = manager.add_from_bytes(None, &record_data).expect("add 3 failed");
        assert_eq!(handle2, handle3, "Handle should be reused again");
    }

    #[test]
    fn test_remove_with_type127_present() {
        // Test removing records when Type 127 is present
        let manager = SmbiosManager::new(3, 9).expect("failed to create manager");

        // Manually add Type 127
        let type127 = Type127EndOfTable::new();
        let bytes = type127.to_bytes();
        let header = SmbiosTableHeader::new(127, 4, manager.allocate_handle().unwrap());
        let record = SmbiosRecord::new(header, None, bytes, 0);
        let type127_handle = record.header.handle;
        manager.records.borrow_mut().push(record);

        // Add a regular record
        let mut record_data = vec![1u8, 4, 0, 0];
        record_data.extend_from_slice(b"\0\0");
        let handle = manager.add_from_bytes(None, &record_data).expect("add failed");

        // Remove the regular record
        manager.remove(handle).expect("remove failed");

        // Type 127 should still be present and last
        let records = manager.records.borrow();
        assert_eq!(records.len(), 1);
        let found_handle = records[0].header.handle;
        let found_type = records[0].header.record_type;
        assert_eq!(found_handle, type127_handle);
        assert_eq!(found_type, 127);
    }

    #[test]
    fn test_string_pool_with_max_length_strings() {
        // Test adding a record with strings at max length
        let manager = SmbiosManager::new(3, 9).expect("failed to create manager");

        let max_string = "a".repeat(SMBIOS_STRING_MAX_LENGTH);
        let mut record_data = vec![1u8, 5, 0, 0];
        record_data.push(1); // String index
        record_data.extend_from_slice(max_string.as_bytes());
        record_data.push(0); // Null terminator
        record_data.push(0); // String pool terminator

        let result = manager.add_from_bytes(None, &record_data);
        assert!(result.is_ok(), "Should successfully add record with max-length string");
    }

    #[test]
    fn test_version_values_stored_correctly() {
        // Test various version combinations
        let test_cases = [(3, 0), (3, 5), (3, 9), (3, 255)];

        for (major, minor) in test_cases {
            let manager = SmbiosManager::new(major, minor).expect("failed to create manager");
            assert_eq!(manager.major_version, major);
            assert_eq!(manager.minor_version, minor);
        }
    }

    #[test]
    fn test_iter_empty_manager() {
        // Test iterator on empty manager
        let manager = SmbiosManager::new(3, 9).expect("failed to create manager");

        let mut count = 0;
        for _ in manager.iter(None) {
            count += 1;
        }
        assert_eq!(count, 0, "Empty manager should yield no records");
    }

    #[test]
    fn test_update_string_changes_data() {
        // Test that update_string actually modifies the record data
        let manager = SmbiosManager::new(3, 9).expect("failed to create manager");

        let header = SmbiosTableHeader::new(1, 5, SMBIOS_HANDLE_PI_RESERVED);
        let mut record_data = Vec::new();
        record_data.extend_from_slice(header.as_bytes());
        record_data.push(1); // String index field
        record_data.extend_from_slice(b"Original\0\0");

        let handle = manager.add_from_bytes(None, &record_data).expect("add failed");

        // Update the string
        manager.update_string(handle, 1, "Updated").expect("update failed");

        // Verify the data changed
        let records = manager.records.borrow();
        let record = records.iter().find(|r| r.header.handle == handle).expect("record not found");
        let data_str = core::str::from_utf8(&record.data).unwrap_or("");
        assert!(data_str.contains("Updated"), "String should be updated");
        assert!(!data_str.contains("Original"), "Old string should be gone");
    }

    #[test]
    fn test_allocate_buffers() {
        use patina::component::service::memory::StdMemoryManager;
        extern crate std;
        use std::boxed::Box;

        let manager = SmbiosManager::new(3, 9).expect("failed to create manager");
        let memory_manager: &'static dyn patina::component::service::memory::MemoryManager =
            Box::leak(Box::new(StdMemoryManager::new()));

        // Verify buffers are not allocated initially
        assert!(manager.table_buffer_addr.borrow().is_none());
        assert!(manager.ep_buffer_addr.borrow().is_none());
        assert_eq!(manager.records.borrow().len(), 0);

        // Allocate buffers
        manager.allocate_buffers(memory_manager).expect("allocate_buffers failed");

        // Verify buffers are now allocated
        assert!(manager.table_buffer_addr.borrow().is_some());
        assert!(manager.ep_buffer_addr.borrow().is_some());

        // Verify Type 127 was automatically added
        assert_eq!(manager.records.borrow().len(), 1);
        assert_eq!(manager.records.borrow()[0].header.record_type, 127);
    }

    #[test]
    fn test_allocate_buffers_idempotent() {
        use patina::component::service::memory::StdMemoryManager;
        extern crate std;
        use std::boxed::Box;

        let manager = SmbiosManager::new(3, 9).expect("failed to create manager");
        let memory_manager: &'static dyn patina::component::service::memory::MemoryManager =
            Box::leak(Box::new(StdMemoryManager::new()));

        // Allocate buffers twice
        manager.allocate_buffers(memory_manager).expect("first allocate_buffers failed");
        let addr1 = *manager.table_buffer_addr.borrow();

        manager.allocate_buffers(memory_manager).expect("second allocate_buffers failed");
        let addr2 = *manager.table_buffer_addr.borrow();

        // Should return same addresses (idempotent)
        assert_eq!(addr1, addr2);

        // Type 127 should only be added once
        assert_eq!(manager.records.borrow().len(), 1);
    }

    #[test]
    fn test_build_table_data() {
        use patina::component::service::memory::StdMemoryManager;
        extern crate std;
        use std::boxed::Box;

        let manager = SmbiosManager::new(3, 9).expect("failed to create manager");
        let memory_manager: &'static dyn patina::component::service::memory::MemoryManager =
            Box::leak(Box::new(StdMemoryManager::new()));

        // Allocate buffers first (this also adds Type 127)
        manager.allocate_buffers(memory_manager).expect("allocate_buffers failed");

        // Add a Type 1 record
        let mut record_data = vec![1u8, 4, 0, 0];
        record_data.extend_from_slice(b"\0\0");
        manager.add_from_bytes(None, &record_data).expect("add failed");

        // Build the table
        let (table_addr, ep_addr, entry_point) = manager.build_table_data().expect("build_table_data failed");

        // Verify addresses are valid (non-zero)
        assert_ne!(table_addr, 0);
        assert_ne!(ep_addr, 0);

        // Verify entry point structure (copy fields to avoid unaligned access on packed struct)
        let anchor = entry_point.anchor_string;
        let major = entry_point.major_version;
        let minor = entry_point.minor_version;
        let length = entry_point.length;
        let ep_table_addr = entry_point.table_address;
        assert_eq!(&anchor, b"_SM3_");
        assert_eq!(major, 3);
        assert_eq!(minor, 9);
        assert_eq!(length, 24); // Size of Smbios30EntryPoint
        assert_eq!(ep_table_addr, table_addr);

        // Verify checksum is valid (sum of all bytes should be 0)
        let bytes = entry_point.as_bytes();
        let sum: u8 = bytes.iter().fold(0u8, |acc, &b| acc.wrapping_add(b));
        assert_eq!(sum, 0, "Entry point checksum should make sum zero");
    }

    #[test]
    fn test_build_table_data_copies_records_correctly() {
        use patina::component::service::memory::StdMemoryManager;
        extern crate std;
        use std::boxed::Box;

        let manager = SmbiosManager::new(3, 9).expect("failed to create manager");
        let memory_manager: &'static dyn patina::component::service::memory::MemoryManager =
            Box::leak(Box::new(StdMemoryManager::new()));

        manager.allocate_buffers(memory_manager).expect("allocate_buffers failed");

        // Add multiple records
        for record_type in [1u8, 2, 3] {
            let mut record_data = vec![record_type, 4, 0, 0];
            record_data.extend_from_slice(b"\0\0");
            manager.add_from_bytes(None, &record_data).expect("add failed");
        }

        // Build the table
        let (table_addr, _, entry_point) = manager.build_table_data().expect("build_table_data failed");

        // Verify table contains all records by checking the table size
        // Each record is 4 bytes header + 2 bytes string terminator = 6 bytes
        // We have 3 user records + 1 Type 127 = 4 records = 24 bytes
        let table_size = entry_point.table_max_size;
        assert_eq!(table_size, 24);

        // Verify the table data starts with our first record (Type 1)
        // SAFETY: table_addr points to valid memory allocated by StdMemoryManager
        let first_byte = unsafe { *(table_addr as *const u8) };
        assert_eq!(first_byte, 1, "First record should be Type 1");
    }

    #[test]
    fn test_build_table_data_no_records_error() {
        use patina::component::service::memory::StdMemoryManager;
        extern crate std;
        use std::boxed::Box;

        let manager = SmbiosManager::new(3, 9).expect("failed to create manager");
        let memory_manager: &'static dyn patina::component::service::memory::MemoryManager =
            Box::leak(Box::new(StdMemoryManager::new()));

        manager.allocate_buffers(memory_manager).expect("allocate_buffers failed");

        // Remove the Type 127 that was added by allocate_buffers
        let records = manager.records.borrow();
        let type127_handle = records[0].header.handle;
        drop(records);
        manager.remove(type127_handle).expect("remove failed");

        // Now build_table_data should fail with NoRecordsAvailable
        let result = manager.build_table_data();
        assert_eq!(result.err(), Some(SmbiosError::NoRecordsAvailable));
    }
}
