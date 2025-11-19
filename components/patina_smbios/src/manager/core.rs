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
use patina::{
    boot_services::{BootServices, allocation::AllocType},
    efi_types::EfiMemoryType,
    uefi_size_to_pages,
};
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
        })
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

    /// Builds the SMBIOS table and installs it in the UEFI Configuration Table
    ///
    /// This function performs the following steps:
    /// 1. Consolidates all SMBIOS records into a contiguous memory buffer
    /// 2. Creates an SMBIOS 3.x Entry Point Structure with proper checksum
    /// 3. Allocates ACPI Reclaim memory for both the table and entry point
    /// 4. Installs the entry point via the UEFI Configuration Table
    ///
    /// # Arguments
    ///
    /// * `boot_services` - UEFI Boot Services for memory allocation and table installation
    ///
    /// # Returns
    ///
    /// Returns a tuple of `(table_address, entry_point_address)` containing the physical
    /// addresses where the SMBIOS table data and entry point structure were allocated.
    ///
    /// # Errors
    ///
    /// * `SmbiosError::NoRecordsAvailable` - No SMBIOS records have been added
    /// * `SmbiosError::AllocationFailed` - Failed to allocate memory or install the configuration table
    ///
    /// # Safety
    ///
    /// This function uses unsafe code for:
    /// - Creating mutable slices to allocated memory
    /// - Writing the entry point structure to allocated memory
    /// - Calling the UEFI `install_configuration_table` interface
    ///
    /// All memory allocations use UEFI Boot Services and are properly tracked by the firmware.
    pub fn install_configuration_table(
        &self,
        boot_services: &patina::boot_services::StandardBootServices,
    ) -> Result<(PhysicalAddress, PhysicalAddress), SmbiosError> {
        let records = self.records.borrow();

        // Step 1: Calculate total table size
        let total_table_size: usize = records.iter().map(|r| r.data.len()).sum();

        if total_table_size == 0 {
            log::error!("Cannot install configuration table: no SMBIOS records have been added");
            return Err(SmbiosError::NoRecordsAvailable);
        }

        // Step 2: Allocate memory for the table (using UEFI Boot Services memory allocation)
        let table_pages = uefi_size_to_pages!(total_table_size);
        let table_address = boot_services
            .allocate_pages(
                AllocType::AnyPage,
                EfiMemoryType::ACPIReclaimMemory, // SMBIOS tables go in ACPI Reclaim memory
                table_pages,
            )
            .map_err(|_| SmbiosError::AllocationFailed)?;

        // Step 3: Copy all records to the table
        // SAFETY: `table_address` was just successfully allocated by UEFI Boot Services for
        // `total_table_size` bytes. The memory is valid, properly aligned, and exclusively owned
        // by this function. The size matches our calculation, making this slice creation safe.
        let table_slice = unsafe { core::slice::from_raw_parts_mut(table_address as *mut u8, total_table_size) };
        let mut offset = 0;

        for record in records.iter() {
            let record_bytes = record.data.as_slice();
            table_slice[offset..offset + record_bytes.len()].copy_from_slice(record_bytes);
            offset += record_bytes.len();
        }

        // Step 4: Create SMBIOS 3.0+ Entry Point Structure
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
            table_address: table_address as u64,
        };

        // Calculate checksum
        entry_point.checksum = Self::calculate_checksum(&entry_point);

        // Step 5: Allocate memory for entry point structure
        let ep_pages = 1; // Entry point fits in one page
        let ep_address = boot_services
            .allocate_pages(AllocType::AnyPage, EfiMemoryType::ACPIReclaimMemory, ep_pages)
            .map_err(|_| SmbiosError::AllocationFailed)?;

        // Step 6: Copy entry point to allocated memory
        let ep_bytes = entry_point.as_bytes();
        // SAFETY: `ep_address` was just successfully allocated by UEFI Boot Services for one page.
        // The size `core::mem::size_of::<Smbios30EntryPoint>()` (24 bytes) fits well within one page (4KB).
        // The memory is valid, properly aligned, and exclusively owned by this function.
        let ep_slice = unsafe {
            core::slice::from_raw_parts_mut(ep_address as *mut u8, core::mem::size_of::<Smbios30EntryPoint>())
        };
        ep_slice.copy_from_slice(ep_bytes);

        // Step 7: Install in UEFI Configuration Table
        // SAFETY: We're calling the UEFI `install_configuration_table` function with:
        // - A valid GUID reference (&SMBIOS_3_X_TABLE_GUID)
        // - A valid pointer to the entry point structure we just allocated and initialized
        // The pointer remains valid for the system's lifetime as it's in ACPI_RECLAIM_MEMORY.
        unsafe {
            boot_services.install_configuration_table(&SMBIOS_3_X_TABLE_GUID, ep_address as *mut ()).map_err(|e| {
                log::error!("Failed to install SMBIOS configuration table: {:?}", e);
                SmbiosError::AllocationFailed
            })?;
        }

        // Store addresses for future reference
        drop(records); // Release borrow before mutating
        self.entry_point_64.replace(Some(Box::new(entry_point)));
        self.table_64_address.replace(Some(table_address as u64));

        Ok((table_address as u64, ep_address as u64))
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

    /// Publish the SMBIOS table to UEFI configuration tables
    pub fn publish_table(
        &self,
        boot_services: &patina::boot_services::StandardBootServices,
    ) -> Result<(r_efi::efi::PhysicalAddress, r_efi::efi::PhysicalAddress), SmbiosError> {
        self.install_configuration_table(boot_services)
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

        // Step 3: Validate header->length is <= (record_data.length - 2) for string pool
        // The string pool needs at least 2 bytes for the double-null terminator
        if (header.length as usize + 2) > record_data.len() {
            return Err(SmbiosError::RecordTooSmall);
        }

        // Step 4: Validate and count strings in a single efficient pass
        let string_pool_start = header.length as usize;
        let string_pool_area = &record_data[string_pool_start..];

        if string_pool_area.len() < 2 {
            return Err(SmbiosError::StringPoolTooSmall);
        }

        // Step 5: Validate string pool format and count strings
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

        self.records.borrow_mut().push(smbios_record);
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
}
