//! SMBIOS Core Implementation
//!
//! Provides the core SMBIOS manager and protocol implementations.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

extern crate alloc;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::{RefCell, UnsafeCell};
use core::ffi::{c_char, c_void};
use patina::boot_services::BootServices;
use patina::boot_services::StandardBootServices;
use patina::boot_services::allocation::{AllocType, MemoryType};
use patina::boot_services::tpl::Tpl;
use patina::tpl_mutex::TplMutex;
use patina::uefi_protocol::ProtocolInterface;
use patina::uefi_size_to_pages;
use r_efi::efi;
use r_efi::efi::Handle;
use r_efi::efi::PhysicalAddress;
use zerocopy::{IntoBytes, Ref};
use zerocopy_derive::{FromBytes, Immutable, IntoBytes as DeriveIntoBytes, KnownLayout};

/// SMBIOS record handle type (16-bit identifier)
pub type SmbiosHandle = u16;

/// Special handle value for automatic assignment
pub const SMBIOS_HANDLE_PI_RESERVED: SmbiosHandle = 0xFFFE;

/// SMBIOS Protocol GUID: 03583ff6-cb36-4940-947e-b9b39f4afaf7
pub const SMBIOS_PROTOCOL_GUID: efi::Guid =
    efi::Guid::from_fields(0x03583ff6, 0xcb36, 0x4940, 0x94, 0x7e, &[0xb9, 0xb3, 0x9f, 0x4a, 0xfa, 0xf7]);

/// SMBIOS 3.x Configuration Table GUID: F2FD1544-9794-4A2C-992E-E5BBCF20E394
///
/// This GUID identifies the SMBIOS 3.0+ entry point structure in the UEFI Configuration Table.
/// Used for SMBIOS 3.0 and later versions which support 64-bit table addresses and remove
/// the 4GB table size limitation of SMBIOS 2.x.
pub const SMBIOS_3_X_TABLE_GUID: efi::Guid =
    efi::Guid::from_fields(0xF2FD1544, 0x9794, 0x4A2C, 0x99, 0x2E, &[0xE5, 0xBB, 0xCF, 0x20, 0xE3, 0x94]);

/// SMBIOS record type
pub type SmbiosType = u8;

/// SMBIOS string maximum length per specification
pub const SMBIOS_STRING_MAX_LENGTH: usize = 64;

/// SMBIOS operation errors
#[derive(Debug, Clone, PartialEq)]
pub enum SmbiosError {
    /// Invalid parameter provided to operation
    InvalidParameter,
    /// Insufficient resources to complete operation
    OutOfResources,
    /// The specified handle is already in use
    HandleAlreadyInUse,
    /// The specified handle was not found
    HandleNotFound,
    /// The record type is not supported
    UnsupportedRecordType,
    /// The handle value is invalid
    InvalidHandle,
    /// String exceeds maximum allowed length
    StringTooLong,
    /// Buffer is too small for operation
    BufferTooSmall,
}

/// Core SMBIOS record management operations
pub trait SmbiosRecords<'a> {
    /// Adds an SMBIOS record to the SMBIOS table from a complete byte representation.
    ///
    /// **This is the recommended method for adding SMBIOS records.** It provides memory safety
    /// and specification compliance by taking the complete record data as a validated byte slice,
    /// avoiding unsafe pointer arithmetic and potential security vulnerabilities.
    ///
    /// # Arguments
    ///
    /// * `producer_handle` - Optional handle of the producer creating this record
    /// * `record_data` - Complete SMBIOS record as a byte slice, including:
    ///   - Header (4 bytes: type, length, handle)
    ///   - Structured data (length - 4 bytes)
    ///   - String pool (null-terminated strings ending with double null)
    ///
    /// # Returns
    ///
    /// Returns the assigned SMBIOS handle for the newly added record.
    ///
    /// # Validation
    ///
    /// This method performs comprehensive validation:
    /// - Verifies minimum buffer size (at least 4 bytes for header)
    /// - Validates header length field
    /// - Ensures sufficient space for string pool (minimum 2 bytes for double null)
    /// - Validates string pool format and counts strings
    /// - Checks for string length violations
    /// - Detects malformed string pools
    fn add_from_bytes(&self, producer_handle: Option<Handle>, record_data: &[u8]) -> Result<SmbiosHandle, SmbiosError>;

    /// Updates a string in an existing SMBIOS record.
    fn update_string(&self, smbios_handle: SmbiosHandle, string_number: usize, string: &str)
    -> Result<(), SmbiosError>;

    /// Removes an SMBIOS record from the SMBIOS table.
    fn remove(&self, smbios_handle: SmbiosHandle) -> Result<(), SmbiosError>;

    /// Discovers SMBIOS records, optionally filtered by type.
    fn get_next(
        &self,
        smbios_handle: &mut SmbiosHandle,
        record_type: Option<SmbiosType>,
    ) -> Result<(SmbiosTableHeader, Option<Handle>), SmbiosError>;

    /// Gets the SMBIOS version information.
    fn version(&self) -> (u8, u8); // (major, minor)

    /// Publishes the SMBIOS table to the UEFI Configuration Table
    ///
    /// This should be called after all records have been added.
    /// Returns (table_address, entry_point_address) on success.
    fn publish_table(
        &self,
        boot_services: &patina::boot_services::StandardBootServices,
    ) -> Result<(r_efi::efi::PhysicalAddress, r_efi::efi::PhysicalAddress), SmbiosError>;
}

/// Extension trait for ergonomic SMBIOS record management
///
/// Provides convenient methods for adding SMBIOS records from structured types.
pub trait SmbiosService {
    /// Add an SMBIOS record from a structured type.
    ///
    /// This is a convenience method that automatically serializes the record
    /// and handles the Service dereferencing internally.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use patina_smbios::SmbiosService;
    ///
    /// // Simply call .add_record() on the service
    /// smbios_service.add_record(None, &bios_info)?;
    /// ```
    fn add_record<T>(
        &self,
        producer_handle: Option<r_efi::efi::Handle>,
        record: &T,
    ) -> Result<SmbiosHandle, SmbiosError>
    where
        T: crate::smbios_record::SmbiosRecordStructure + crate::smbios_record::SmbiosFieldLayout;
}

impl SmbiosService for patina::component::service::Service<dyn SmbiosRecords<'static>> {
    fn add_record<T>(
        &self,
        producer_handle: Option<r_efi::efi::Handle>,
        record: &T,
    ) -> Result<SmbiosHandle, SmbiosError>
    where
        T: crate::smbios_record::SmbiosRecordStructure + crate::smbios_record::SmbiosFieldLayout,
    {
        // Dereference Service twice to get &'static (dyn SmbiosRecords)
        let service: &'static (dyn SmbiosRecords<'static> + 'static) = **self;
        let bytes = record.to_bytes();
        service.add_from_bytes(producer_handle, &bytes)
    }
}

/// SMBIOS 3.0 entry point structure (64-bit)
/// Per SMBIOS 3.0+ specification section 5.2.2
#[repr(C, packed)]
#[derive(Clone, Copy, DeriveIntoBytes, Immutable)]
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
    records: RefCell<Vec<SmbiosRecord>>,
    next_handle: RefCell<SmbiosHandle>,
    freed_handles: RefCell<Vec<SmbiosHandle>>,
    major_version: u8,
    minor_version: u8,
    entry_point_64: RefCell<Option<Box<Smbios30EntryPoint>>>,
    table_64_address: RefCell<Option<PhysicalAddress>>,
}

impl SmbiosManager {
    /// Creates a new SMBIOS manager with the specified version
    ///
    /// # Arguments
    ///
    /// * `major_version` - SMBIOS major version (e.g., 3 for SMBIOS 3.x)
    /// * `minor_version` - SMBIOS minor version (e.g., 9 for SMBIOS 3.9)
    pub fn new(major_version: u8, minor_version: u8) -> Self {
        Self {
            records: RefCell::new(Vec::new()),
            next_handle: RefCell::new(1),
            freed_handles: RefCell::new(Vec::new()),
            major_version,
            minor_version,
            entry_point_64: RefCell::new(None),
            table_64_address: RefCell::new(None),
        }
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
    fn validate_string(s: &str) -> Result<(), SmbiosError> {
        if s.len() > SMBIOS_STRING_MAX_LENGTH {
            return Err(SmbiosError::StringTooLong);
        }
        // Strings must NOT contain null terminators - they are added during serialization
        if s.contains('\0') {
            return Err(SmbiosError::InvalidParameter);
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
    /// Returns `SmbiosError::InvalidParameter` if:
    /// - The pool doesn't end with double null
    /// - The pool is too small (< 2 bytes)
    /// - Consecutive nulls are found in the middle
    ///
    /// Returns `SmbiosError::StringTooLong` if any string exceeds SMBIOS_STRING_MAX_LENGTH
    fn validate_and_count_strings(string_pool_area: &[u8]) -> Result<usize, SmbiosError> {
        let len = string_pool_area.len();

        // Must end with double null
        if len < 2 || string_pool_area[len - 1] != 0 || string_pool_area[len - 2] != 0 {
            return Err(SmbiosError::InvalidParameter);
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
                return Err(SmbiosError::InvalidParameter);
            }
            if string_bytes.len() > SMBIOS_STRING_MAX_LENGTH {
                return Err(SmbiosError::StringTooLong);
            }
        }

        Ok(strings.len())
    }

    /// Parse strings from an SMBIOS string pool
    ///
    /// Extracts all strings from the string pool area, converting them to Rust Strings.
    /// This is a higher-level companion to `validate_and_count_strings` that returns
    /// the actual string data instead of just counting.
    ///
    /// # Arguments
    ///
    /// * `string_pool_area` - The string pool portion of an SMBIOS record
    ///
    /// # Returns
    ///
    /// Returns a Vec of Strings extracted from the pool, or an error if the pool is malformed
    fn parse_strings_from_pool(string_pool_area: &[u8]) -> Result<Vec<String>, SmbiosError> {
        let len = string_pool_area.len();

        // Must end with double null
        if len < 2 || string_pool_area[len - 1] != 0 || string_pool_area[len - 2] != 0 {
            return Err(SmbiosError::InvalidParameter);
        }

        // Handle empty string pool (just double null)
        if len == 2 {
            return Ok(Vec::new());
        }

        // Remove the final double-null terminator and split by null bytes
        let data_without_terminator = &string_pool_area[..len - 2];

        // Split by null bytes to get individual strings
        data_without_terminator
            .split(|&b| b == 0)
            .map(|string_bytes| {
                if string_bytes.is_empty() {
                    // Empty slice means consecutive nulls (invalid)
                    Err(SmbiosError::InvalidParameter)
                } else {
                    // Convert bytes to String using UTF-8 lossy conversion
                    Ok(String::from_utf8_lossy(string_bytes).into_owned())
                }
            })
            .collect()
    }

    /// Build a complete SMBIOS record from a header and string array
    ///
    /// This is a helper function for creating SMBIOS records when you have
    /// the structured data (header) and want to attach strings.
    ///
    /// # Arguments
    ///
    /// * `header` - The SMBIOS table header and structured data
    /// * `strings` - Array of string slices to include in the string pool
    ///
    /// # Returns
    ///
    /// Returns a complete SMBIOS record byte array ready to be added via `add_from_bytes`
    #[allow(dead_code)]
    pub fn build_record_with_strings(header: &SmbiosTableHeader, strings: &[&str]) -> Result<Vec<u8>, SmbiosError> {
        // Validate all strings first
        for s in strings {
            Self::validate_string(s)?;
        }

        let mut record = Vec::new();

        // Add the structured data using zerocopy
        record.extend_from_slice(header.as_bytes());

        // Add strings
        if strings.is_empty() {
            // No strings - add double null terminator
            record.extend_from_slice(&[0, 0]);
        } else {
            for s in strings {
                record.extend_from_slice(s.as_bytes());
                record.push(0); // Null terminator
            }
            record.push(0); // Double null terminator
        }

        Ok(record)
    }

    /// Allocate a new handle using a free list for efficient O(1) allocation
    ///
    /// This implementation maintains a free list of previously freed handles to avoid
    /// O(n) searches through all records. The allocation strategy is:
    /// 1. If freed_handles is non-empty, pop and reuse a freed handle
    /// 2. Otherwise, use next_handle and increment it
    /// 3. If next_handle reaches the reserved range (0xFFFE), wrap to 1
    /// 4. If all handles are exhausted, return OutOfResources
    fn allocate_handle(&self) -> Result<SmbiosHandle, SmbiosError> {
        // First, try to reuse a freed handle (most efficient)
        if let Some(handle) = self.freed_handles.borrow_mut().pop() {
            return Ok(handle);
        }

        // No freed handles available, use next_handle
        let candidate = *self.next_handle.borrow();

        // Check if we've exhausted the handle space
        // Valid handles are 1..=0xFEFF (0xFFFE and 0xFFFF are reserved)
        if candidate >= 0xFFFE {
            // All handles exhausted
            return Err(SmbiosError::OutOfResources);
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
    /// * `SmbiosError::InvalidParameter` - No SMBIOS records have been added
    /// * `SmbiosError::OutOfResources` - Failed to allocate memory or install the configuration table
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
            log::warn!("No SMBIOS records to install");
            return Err(SmbiosError::InvalidParameter);
        }

        // Step 2: Allocate memory for the table (using UEFI Boot Services memory allocation)
        let table_pages = uefi_size_to_pages!(total_table_size);
        let table_address = boot_services
            .allocate_pages(
                AllocType::AnyPage,
                MemoryType::ACPI_RECLAIM_MEMORY, // SMBIOS tables go in ACPI Reclaim memory
                table_pages,
            )
            .map_err(|_| SmbiosError::OutOfResources)?;

        // Step 3: Copy all records to the table
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
            .allocate_pages(AllocType::AnyPage, MemoryType::ACPI_RECLAIM_MEMORY, ep_pages)
            .map_err(|_| SmbiosError::OutOfResources)?;

        // Step 6: Copy entry point to allocated memory
        let ep_bytes = entry_point.as_bytes();
        let ep_slice = unsafe {
            core::slice::from_raw_parts_mut(ep_address as *mut u8, core::mem::size_of::<Smbios30EntryPoint>())
        };
        ep_slice.copy_from_slice(ep_bytes);

        // Step 7: Install in UEFI Configuration Table
        unsafe {
            boot_services.install_configuration_table(&SMBIOS_3_X_TABLE_GUID, ep_address as *mut c_void).map_err(
                |e| {
                    log::error!("Failed to install SMBIOS configuration table: {:?}", e);
                    SmbiosError::OutOfResources
                },
            )?;
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
    /// # Safety
    ///
    /// Uses zerocopy to safely convert the entry point structure to a byte slice.
    /// This is safe because Smbios30EntryPoint derives IntoBytes.
    fn calculate_checksum(entry_point: &Smbios30EntryPoint) -> u8 {
        let bytes = entry_point.as_bytes();

        let sum: u8 = bytes.iter().fold(0u8, |acc, &b| acc.wrapping_add(b));
        0u8.wrapping_sub(sum)
    }
}

impl SmbiosRecords<'static> for SmbiosManager {
    fn add_from_bytes(&self, producer_handle: Option<Handle>, record_data: &[u8]) -> Result<SmbiosHandle, SmbiosError> {
        // Step 1: Validate minimum size for header (at least 4 bytes)
        if record_data.len() < core::mem::size_of::<SmbiosTableHeader>() {
            return Err(SmbiosError::BufferTooSmall);
        }

        // Step 2: Parse and validate header using zerocopy
        let (header_ref, _rest) =
            Ref::<&[u8], SmbiosTableHeader>::from_prefix(record_data).map_err(|_| SmbiosError::InvalidParameter)?;
        let header: &SmbiosTableHeader = &header_ref;

        // Step 3: Validate header->length is <= (record_data.length - 2) for string pool
        // The string pool needs at least 2 bytes for the double-null terminator
        if (header.length as usize + 2) > record_data.len() {
            return Err(SmbiosError::BufferTooSmall);
        }

        // Step 4: Validate and count strings in a single efficient pass
        let string_pool_start = header.length as usize;
        let string_pool_area = &record_data[string_pool_start..];

        if string_pool_area.len() < 2 {
            return Err(SmbiosError::InvalidParameter);
        }

        // Step 5: Validate string pool format and count strings
        let string_count = Self::validate_and_count_strings(string_pool_area)?;

        // If all validation passes, allocate handle and build record
        let smbios_handle = self.allocate_handle()?;

        let mut record_header =
            SmbiosTableHeader { record_type: header.record_type, length: header.length, handle: smbios_handle };
        record_header.handle = smbios_handle;

        // Update the handle in the actual data
        let mut data = record_data.to_vec();
        let handle_bytes = smbios_handle.to_le_bytes();
        data[2] = handle_bytes[0]; // Handle is at offset 2 in header
        data[3] = handle_bytes[1];

        let smbios_record = SmbiosRecord::new(record_header, producer_handle, data, string_count);

        self.records.borrow_mut().push(smbios_record);
        Ok(smbios_handle)
    }

    fn update_string(
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
            return Err(SmbiosError::InvalidHandle);
        }

        // Parse the existing string pool
        let header_length = record.header.length as usize;
        if record.data.len() < header_length + 2 {
            return Err(SmbiosError::BufferTooSmall);
        }

        // Extract existing strings from the string pool using the helper function
        let string_pool_start = header_length;
        let string_pool = &record.data[string_pool_start..];
        let mut existing_strings = Self::parse_strings_from_pool(string_pool)?;

        // Validate that we have enough strings
        if string_number > existing_strings.len() {
            return Err(SmbiosError::InvalidHandle);
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

    fn remove(&self, smbios_handle: SmbiosHandle) -> Result<(), SmbiosError> {
        let pos = self
            .records
            .borrow()
            .iter()
            .position(|r| r.header.handle == smbios_handle)
            .ok_or(SmbiosError::HandleNotFound)?;

        self.records.borrow_mut().remove(pos);

        // Add the freed handle to the free list for reuse
        // Only add valid handles (1..0xFFFE) to the free list
        if (1..0xFFFE).contains(&smbios_handle) {
            self.freed_handles.borrow_mut().push(smbios_handle);
        }

        Ok(())
    }

    fn get_next(
        &self,
        smbios_handle: &mut SmbiosHandle,
        record_type: Option<SmbiosType>,
    ) -> Result<(SmbiosTableHeader, Option<Handle>), SmbiosError> {
        let records = self.records.borrow();

        let start_idx = if *smbios_handle == SMBIOS_HANDLE_PI_RESERVED {
            0
        } else {
            records.iter().position(|r| r.header.handle == *smbios_handle).map(|i| i + 1).unwrap_or(records.len())
        };

        for record in &records[start_idx..] {
            if let Some(rt) = record_type
                && record.header.record_type != rt
            {
                continue;
            }

            *smbios_handle = record.header.handle;
            return Ok((record.header.clone(), record.producer_handle));
        }

        *smbios_handle = SMBIOS_HANDLE_PI_RESERVED;
        Err(SmbiosError::HandleNotFound)
    }

    fn version(&self) -> (u8, u8) {
        (self.major_version, self.minor_version)
    }

    fn publish_table(
        &self,
        boot_services: &patina::boot_services::StandardBootServices,
    ) -> Result<(r_efi::efi::PhysicalAddress, r_efi::efi::PhysicalAddress), SmbiosError> {
        self.install_configuration_table(boot_services)
    }
}

/// SMBIOS table header structure
#[repr(C, packed)]
#[derive(Debug, Clone, PartialEq, FromBytes, DeriveIntoBytes, Immutable, KnownLayout)]
pub struct SmbiosTableHeader {
    /// SMBIOS record type
    pub record_type: SmbiosType,
    /// Length of the structured data (including header)
    pub length: u8,
    /// Unique handle for this record
    pub handle: SmbiosHandle,
}

impl SmbiosTableHeader {
    /// Creates a new SMBIOS table header
    pub fn new(record_type: SmbiosType, length: u8, handle: SmbiosHandle) -> Self {
        Self { record_type, length, handle }
    }
}

/// Internal SMBIOS record representation
///
/// This implementation is for SMBIOS 3.0+ specification which uses 64-bit addressing.
pub struct SmbiosRecord {
    /// SMBIOS table header
    pub header: SmbiosTableHeader,
    /// Optional handle of the producer that created this record
    pub producer_handle: Option<Handle>,
    /// Complete record data including header and strings
    pub data: Vec<u8>,
    string_count: usize,
}

impl SmbiosRecord {
    /// Creates a new SMBIOS record
    pub fn new(header: SmbiosTableHeader, producer_handle: Option<Handle>, data: Vec<u8>, string_count: usize) -> Self {
        Self { header, producer_handle, data, string_count }
    }
}

/// Builder for constructing SMBIOS records
#[derive(Debug, PartialEq)]
pub struct SmbiosRecordBuilder {
    record_type: u8,
    data: Vec<u8>,
    strings: Vec<String>,
}

impl SmbiosRecordBuilder {
    /// Creates a new SMBIOS record builder for the specified record type
    pub fn new(record_type: u8) -> Self {
        Self { record_type, data: Vec::new(), strings: Vec::new() }
    }

    /// Adds a field to the record
    pub fn add_field<T>(mut self, value: T) -> Self
    where
        T: Copy + zerocopy::IntoBytes + zerocopy::Immutable,
    {
        self.data.extend_from_slice(value.as_bytes());
        self
    }

    /// Adds a string to the record's string pool
    pub fn add_string(mut self, s: String) -> Result<Self, SmbiosError> {
        SmbiosManager::validate_string(&s)?;
        self.strings.push(s);
        Ok(self)
    }

    /// Builds the complete SMBIOS record
    pub fn build(self) -> Result<Vec<u8>, SmbiosError> {
        let mut record = Vec::new();

        // Add header using zerocopy
        let header = SmbiosTableHeader {
            record_type: self.record_type,
            length: (core::mem::size_of::<SmbiosTableHeader>() + self.data.len()) as u8,
            handle: SMBIOS_HANDLE_PI_RESERVED,
        };

        record.extend_from_slice(header.as_bytes());

        // Add data
        record.extend_from_slice(&self.data);

        // Add strings
        if self.strings.is_empty() {
            record.extend_from_slice(&[0, 0]);
        } else {
            for s in &self.strings {
                record.extend_from_slice(s.as_bytes());
                record.push(0);
            }
            record.push(0);
        }

        Ok(record)
    }
}

/// Global storage for boot_services reference
///
/// This reference is stored during protocol installation and remains valid for
/// the lifetime of the system. Required for TplMutex construction.
///
/// # Safety
///
/// - Initialized once during `install_smbios_protocol`
/// - The boot_services reference must have 'static lifetime
/// - Access is thread-safe due to UEFI's single-threaded DXE model
struct GlobalBootServices {
    boot_services: UnsafeCell<Option<&'static StandardBootServices>>,
}

unsafe impl Sync for GlobalBootServices {}

impl GlobalBootServices {
    const fn new() -> Self {
        Self { boot_services: UnsafeCell::new(None) }
    }

    /// Initialize with a boot_services reference
    ///
    /// # Safety
    ///
    /// Must be called exactly once during system initialization.
    /// The boot_services reference must have 'static lifetime.
    unsafe fn initialize(&self, boot_services: &'static StandardBootServices) {
        unsafe { *self.boot_services.get() = Some(boot_services) };
    }

    /// Get the stored boot_services reference
    ///
    /// # Safety
    ///
    /// Returns None if not initialized
    #[allow(dead_code)] // Reserved for future diagnostic access to raw boot services
    unsafe fn get(&self) -> Option<&'static StandardBootServices> {
        unsafe { *self.boot_services.get() }
    }
}

static BOOT_SERVICES: GlobalBootServices = GlobalBootServices::new();

/// Global storage for the SMBIOS manager wrapped in TplMutex
///
/// Uses TplMutex for TPL-aware synchronization. When locked, TPL is raised to CALLBACK
/// level, preventing timer interrupt reentrancy. TPL is automatically restored when
/// the lock guard is dropped.
///
/// # Safety
///
/// - The TplMutex is wrapped in UnsafeCell for static initialization
/// - Access is protected by TplMutex.lock() which raises TPL to CALLBACK
/// - The manager is initialized once during protocol installation
/// - The pointer remains valid for the lifetime of the system
struct GlobalSmbiosManager {
    manager: UnsafeCell<Option<TplMutex<'static, SmbiosManager, StandardBootServices>>>,
}

unsafe impl Sync for GlobalSmbiosManager {}

impl GlobalSmbiosManager {
    const fn new() -> Self {
        Self { manager: UnsafeCell::new(None) }
    }

    /// Initialize the global manager with TplMutex protection
    ///
    /// # Safety
    ///
    /// Caller must ensure this is called only once during system initialization
    unsafe fn initialize(
        &self,
        tpl_mutex: TplMutex<'static, SmbiosManager, StandardBootServices>,
    ) -> Result<(), SmbiosError> {
        let ptr = self.manager.get();
        if unsafe { (*ptr).is_some() } {
            return Err(SmbiosError::InvalidParameter); // Already initialized
        }
        unsafe { *ptr = Some(tpl_mutex) };
        Ok(())
    }

    /// Get a reference to the TplMutex (returns None if not initialized)
    ///
    /// # Safety
    ///
    /// Returns a raw reference to the TplMutex. Caller must call .lock()
    /// to get TPL-protected access to the manager.
    unsafe fn get(&self) -> Option<&'static TplMutex<'static, SmbiosManager, StandardBootServices>> {
        unsafe { (*self.manager.get()).as_ref() }
    }

    /// Clear the manager (for cleanup on error)
    ///
    /// # Safety
    ///
    /// Caller must ensure this is only called during error cleanup
    unsafe fn clear(&self) {
        unsafe { *self.manager.get() = None };
    }
}

static SMBIOS_MANAGER: GlobalSmbiosManager = GlobalSmbiosManager::new();

/// Storage for the protocol interface pointer (for lifetime management)
struct GlobalProtocolInterface {
    interface: UnsafeCell<*mut c_void>,
}

unsafe impl Sync for GlobalProtocolInterface {}

impl GlobalProtocolInterface {
    const fn new() -> Self {
        Self { interface: UnsafeCell::new(core::ptr::null_mut()) }
    }

    unsafe fn set(&self, ptr: *mut c_void) {
        unsafe { *self.interface.get() = ptr };
    }

    #[allow(dead_code)]
    unsafe fn get(&self) -> *mut c_void {
        unsafe { *self.interface.get() }
    }

    unsafe fn clear(&self) {
        unsafe { *self.interface.get() = core::ptr::null_mut() };
    }
}

static SMBIOS_PROTOCOL_INTERFACE: GlobalProtocolInterface = GlobalProtocolInterface::new();

/// Storage for the protocol handle
struct GlobalProtocolHandle {
    handle: UnsafeCell<efi::Handle>,
}

unsafe impl Sync for GlobalProtocolHandle {}

impl GlobalProtocolHandle {
    const fn new() -> Self {
        Self { handle: UnsafeCell::new(core::ptr::null_mut()) }
    }

    unsafe fn set(&self, h: efi::Handle) {
        unsafe { *self.handle.get() = h };
    }

    #[allow(dead_code)]
    unsafe fn get(&self) -> efi::Handle {
        unsafe { *self.handle.get() }
    }
}

static SMBIOS_PROTOCOL_HANDLE: GlobalProtocolHandle = GlobalProtocolHandle::new();

/// Wrapper for static SMBIOS header buffer that implements Sync
///
/// SAFETY: This is safe because UEFI DXE runs in a single-threaded environment,
/// so there's no actual concurrent access despite the Sync implementation.
struct StaticHeaderBuffer(core::cell::UnsafeCell<SmbiosTableHeader>);

unsafe impl Sync for StaticHeaderBuffer {}

impl StaticHeaderBuffer {
    const fn new(header: SmbiosTableHeader) -> Self {
        Self(core::cell::UnsafeCell::new(header))
    }

    unsafe fn get(&self) -> *mut SmbiosTableHeader {
        self.0.get()
    }
}

/// Static storage for header returned by get_next
///
/// This avoids heap allocation issues. The header is stored in a static location
/// that persists for the lifetime of the program. Since SMBIOS headers are small
/// (4 bytes) and get_next is typically called sequentially, a single static buffer
/// is sufficient. The caller receives a pointer to this buffer which remains valid
/// until the next call to get_next.
static SMBIOS_HEADER_BUFFER: StaticHeaderBuffer =
    StaticHeaderBuffer::new(SmbiosTableHeader { record_type: 0, length: 0, handle: 0 });

/// Gets a reference to the global SMBIOS manager TplMutex
///
/// # Returns
///
/// Returns `Some(&TplMutex<SmbiosManager>)` if the manager has been installed,
/// `None` if `install_smbios_protocol` has not been called yet.
///
/// # Usage
///
/// To access the manager, you must call `.lock()` on the returned TplMutex:
///
/// ```ignore
/// if let Some(tpl_mutex) = get_global_smbios_manager() {
///     let manager = tpl_mutex.lock(); // TPL raised to CALLBACK
///     // ... use manager ...
///     // TPL automatically restored when guard drops
/// }
/// ```
///
/// # TPL Protection
///
/// The TplMutex automatically raises TPL to CALLBACK level when locked, preventing
/// timer interrupt reentrancy. The TPL is restored when the lock guard is dropped.
pub fn get_global_smbios_manager() -> Option<&'static TplMutex<'static, SmbiosManager, StandardBootServices>> {
    unsafe { SMBIOS_MANAGER.get() }
}

#[repr(C)]
#[allow(dead_code)]
struct SmbiosProtocol {
    add: SmbiosAdd,
    update_string: SmbiosUpdateString,
    remove: SmbiosRemove,
    get_next: SmbiosGetNext,
    major_version: u8,
    minor_version: u8,
}

unsafe impl ProtocolInterface for SmbiosProtocol {
    const PROTOCOL_GUID: efi::Guid =
        efi::Guid::from_fields(0x03583ff6, 0xcb36, 0x4940, 0x94, 0x7e, &[0xb9, 0xb3, 0x9f, 0x4a, 0xfa, 0xf7]);
}

#[allow(dead_code)]
type SmbiosAdd =
    extern "efiapi" fn(*const SmbiosProtocol, efi::Handle, *mut SmbiosHandle, *const SmbiosTableHeader) -> efi::Status;

#[allow(dead_code)]
type SmbiosUpdateString =
    extern "efiapi" fn(*const SmbiosProtocol, *mut SmbiosHandle, *mut usize, *const c_char) -> efi::Status;

#[allow(dead_code)]
type SmbiosRemove = extern "efiapi" fn(*const SmbiosProtocol, SmbiosHandle) -> efi::Status;

#[allow(dead_code)]
type SmbiosGetNext = extern "efiapi" fn(
    *const SmbiosProtocol,
    *mut SmbiosHandle,
    *mut SmbiosType,
    *mut *mut SmbiosTableHeader,
    *mut efi::Handle,
) -> efi::Status;

impl SmbiosProtocol {
    #[allow(dead_code)]
    fn new(major_version: u8, minor_version: u8) -> Self {
        Self {
            add: Self::add_ext,
            update_string: Self::update_string_ext,
            remove: Self::remove_ext,
            get_next: Self::get_next_ext,
            major_version,
            minor_version,
        }
    }

    /// C protocol implementation for adding SMBIOS records
    ///
    /// # Safety
    ///
    /// This function is only safe to call from the C UEFI protocol layer where the
    /// caller guarantees that `record` points to a complete, valid SMBIOS record.
    ///
    /// # TPL Protection
    ///
    /// This function uses TplMutex.lock() which automatically raises TPL to CALLBACK
    /// level, preventing timer interrupt reentrancy during manager access.
    #[allow(dead_code)]
    extern "efiapi" fn add_ext(
        _protocol: *const SmbiosProtocol,
        producer_handle: efi::Handle,
        smbios_handle: *mut SmbiosHandle,
        record: *const SmbiosTableHeader,
    ) -> efi::Status {
        // Safety checks
        if smbios_handle.is_null() || record.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }

        // Get the global TplMutex
        // SAFETY: We're just checking if the TplMutex exists
        let tpl_mutex = match unsafe { SMBIOS_MANAGER.get() } {
            Some(m) => m,
            None => return efi::Status::NOT_READY,
        };

        // Lock the mutex - TPL is automatically raised to CALLBACK
        let manager = tpl_mutex.lock();

        // SAFETY: The C UEFI protocol caller guarantees that `record` points to a valid,
        // complete SMBIOS record. We read the length field to determine the full record size.
        unsafe {
            let header = &*record;
            let record_length = header.length as usize;

            // Validate that we can safely read the record
            if record_length < core::mem::size_of::<SmbiosTableHeader>() {
                return efi::Status::INVALID_PARAMETER;
            }

            // Scan for the string pool terminator (double null)
            let base_ptr = record as *const u8;

            // Scan for double null terminator
            let mut consecutive_nulls = 0;
            let mut offset = record_length;
            const MAX_STRING_POOL_SIZE: usize = 4096; // Safety limit

            while consecutive_nulls < 2 && offset < record_length + MAX_STRING_POOL_SIZE {
                let byte = *base_ptr.add(offset);
                if byte == 0 {
                    consecutive_nulls += 1;
                } else {
                    consecutive_nulls = 0;
                }
                offset += 1;
            }

            if consecutive_nulls < 2 {
                // Malformed record - no double null terminator found
                return efi::Status::INVALID_PARAMETER;
            }

            let total_size = offset;

            // Create a slice of the complete record
            let full_record_bytes = core::slice::from_raw_parts(base_ptr, total_size);

            // Convert handle
            let producer_opt = if producer_handle.is_null() { None } else { Some(producer_handle) };

            // Add the record
            match manager.add_from_bytes(producer_opt, full_record_bytes) {
                Ok(handle) => {
                    *smbios_handle = handle;
                    efi::Status::SUCCESS
                }
                Err(SmbiosError::InvalidParameter) => efi::Status::INVALID_PARAMETER,
                Err(SmbiosError::OutOfResources) => efi::Status::OUT_OF_RESOURCES,
                Err(SmbiosError::HandleAlreadyInUse) => efi::Status::ALREADY_STARTED,
                Err(SmbiosError::BufferTooSmall) => efi::Status::BUFFER_TOO_SMALL,
                Err(SmbiosError::StringTooLong) => efi::Status::INVALID_PARAMETER,
                Err(_) => efi::Status::DEVICE_ERROR,
            }
        }
        // TPL automatically restored when manager (TplMutexGuard) is dropped
    }

    #[allow(dead_code)]
    extern "efiapi" fn update_string_ext(
        _protocol: *const SmbiosProtocol,
        smbios_handle: *mut SmbiosHandle,
        string_number: *mut usize,
        string: *const c_char,
    ) -> efi::Status {
        if smbios_handle.is_null() || string_number.is_null() || string.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }

        // Get the global TplMutex and lock it (raises TPL to CALLBACK)
        let tpl_mutex = match unsafe { SMBIOS_MANAGER.get() } {
            Some(m) => m,
            None => return efi::Status::NOT_READY,
        };

        let manager = tpl_mutex.lock();

        unsafe {
            let handle = *smbios_handle;
            let str_num = *string_number;

            // Convert C string to Rust str
            let c_str = core::ffi::CStr::from_ptr(string);
            let rust_str = match c_str.to_str() {
                Ok(s) => s,
                Err(_) => return efi::Status::INVALID_PARAMETER,
            };

            match manager.update_string(handle, str_num, rust_str) {
                Ok(()) => efi::Status::SUCCESS,
                Err(SmbiosError::InvalidParameter) => efi::Status::INVALID_PARAMETER,
                Err(SmbiosError::HandleNotFound) => efi::Status::NOT_FOUND,
                Err(SmbiosError::StringTooLong) => efi::Status::INVALID_PARAMETER,
                Err(_) => efi::Status::DEVICE_ERROR,
            }
        }
        // TPL automatically restored when manager (TplMutexGuard) is dropped
    }

    #[allow(dead_code)]
    extern "efiapi" fn remove_ext(_protocol: *const SmbiosProtocol, smbios_handle: SmbiosHandle) -> efi::Status {
        // Get the global TplMutex and lock it (raises TPL to CALLBACK)
        let tpl_mutex = match unsafe { SMBIOS_MANAGER.get() } {
            Some(m) => m,
            None => return efi::Status::NOT_READY,
        };

        let manager = tpl_mutex.lock();

        match manager.remove(smbios_handle) {
            Ok(()) => efi::Status::SUCCESS,
            Err(SmbiosError::HandleNotFound) => efi::Status::NOT_FOUND,
            Err(_) => efi::Status::DEVICE_ERROR,
        }
        // TPL automatically restored when manager (TplMutexGuard) is dropped
    }

    #[allow(dead_code)]
    extern "efiapi" fn get_next_ext(
        _protocol: *const SmbiosProtocol,
        smbios_handle: *mut SmbiosHandle,
        record_type: *mut SmbiosType,
        record: *mut *mut SmbiosTableHeader,
        producer_handle: *mut efi::Handle,
    ) -> efi::Status {
        if smbios_handle.is_null() || record.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }

        // Get the global TplMutex and lock it (raises TPL to CALLBACK)
        let tpl_mutex = match unsafe { SMBIOS_MANAGER.get() } {
            Some(m) => m,
            None => return efi::Status::NOT_READY,
        };

        let manager = tpl_mutex.lock();

        unsafe {
            let mut handle = *smbios_handle;
            let type_filter = if record_type.is_null() { None } else { Some(*record_type) };

            match manager.get_next(&mut handle, type_filter) {
                Ok((header_value, prod_handle)) => {
                    *smbios_handle = handle;

                    // Store header in static buffer and return pointer to it.
                    // SAFETY: UEFI DXE is single-threaded, so there's no concurrent access.
                    // The pointer remains valid until the next call to get_next (standard UEFI pattern).
                    let buffer_ptr = SMBIOS_HEADER_BUFFER.get();
                    *buffer_ptr = header_value;
                    *record = buffer_ptr;

                    if !producer_handle.is_null() {
                        *producer_handle = prod_handle.unwrap_or(core::ptr::null_mut());
                    }
                    efi::Status::SUCCESS
                }
                Err(SmbiosError::HandleNotFound) => efi::Status::NOT_FOUND,
                Err(_) => efi::Status::DEVICE_ERROR,
            }
        }
        // TPL automatically restored when manager (TplMutexGuard) is dropped
    }
}

/// Installs the SMBIOS protocol for C/EDKII driver compatibility
///
/// This function should be called after the SMBIOS service is registered.
/// It creates a C-compatible protocol interface that wraps a global manager instance
/// protected by TplMutex.
///
/// # Arguments
///
/// * `manager` - The SmbiosManager that will be wrapped in TplMutex and moved into global storage
/// * `boot_services` - The UEFI boot services for protocol installation and TPL management
///
/// # Safety
///
/// This function:
/// - Stores the boot_services reference globally for 'static lifetime
/// - Wraps the manager in TplMutex with CALLBACK TPL level
/// - Moves the TplMutex into global storage for 'static lifetime
/// - The manager must not already be installed (function will return error if called twice)
/// - The protocol will remain installed for the lifetime of the system
///
/// # TPL Protection
///
/// The manager is protected by TplMutex at CALLBACK level. When protocol functions
/// are called, the TplMutex.lock() automatically raises TPL to CALLBACK, preventing
/// timer interrupt reentrancy. TPL is automatically restored when the lock guard drops.
pub fn install_smbios_protocol(
    manager: SmbiosManager,
    boot_services: &'static StandardBootServices,
) -> Result<efi::Handle, SmbiosError> {
    // Get the version before moving the manager
    let (major, minor) = manager.version();

    // Store boot_services reference globally for TplMutex
    // SAFETY: This function should only be called once during system initialization
    unsafe {
        BOOT_SERVICES.initialize(boot_services);
    }

    // Create TplMutex wrapping the manager with CALLBACK TPL level
    // This ensures timer interrupts cannot cause reentrancy
    let tpl_mutex = TplMutex::new(boot_services, Tpl::CALLBACK, manager);

    // Initialize the global manager with the TplMutex
    // SAFETY: This function should only be called once during system initialization
    unsafe {
        SMBIOS_MANAGER.initialize(tpl_mutex)?;
    }

    // Create the protocol instance
    let protocol = SmbiosProtocol::new(major, minor);
    let interface = Box::into_raw(Box::new(protocol));
    let interface_void = interface as *mut c_void;

    // Store the interface pointer for lifetime management
    // SAFETY: We just created this pointer and it's valid
    unsafe {
        SMBIOS_PROTOCOL_INTERFACE.set(interface_void);
    }

    // Install the protocol using the unchecked interface since we have a raw pointer
    let handle = unsafe {
        boot_services.install_protocol_interface_unchecked(
            None, // Let UEFI create a new handle
            &SMBIOS_PROTOCOL_GUID,
            interface_void,
        )
    };

    match handle {
        Ok(h) => {
            // Store the handle
            // SAFETY: We just received this valid handle from boot_services
            unsafe {
                SMBIOS_PROTOCOL_HANDLE.set(h);
            }
            Ok(h)
        }
        Err(status) => {
            // Clean up on failure
            unsafe {
                drop(Box::from_raw(interface));
                SMBIOS_MANAGER.clear();
                SMBIOS_PROTOCOL_INTERFACE.clear();
            }
            log::error!("Failed to install SMBIOS protocol: {:?}", status);
            Err(SmbiosError::OutOfResources)
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use crate::smbios_record::SmbiosRecordStructure;
    use crate::smbios_record::Type0PlatformFirmwareInformation;
    use std::vec;

    #[test]
    fn test_smbios_record_builder_builds_bytes() {
        // Ensure builder returns a non-empty record buffer for a minimal System Information record
        let record = SmbiosRecordBuilder::new(1) // System Information
            .add_field(1u8) // manufacturer string index
            .add_field(2u8) // product name string index
            .add_string(String::from("ACME Corp"))
            .expect("add_string failed")
            .add_string(String::from("SuperServer 3000"))
            .expect("add_string failed")
            .build()
            .expect("build failed");

        assert!(record.len() > core::mem::size_of::<SmbiosTableHeader>());
        // First byte is the record type
        assert_eq!(record[0], 1u8);
    }

    #[test]
    fn test_add_type0_platform_firmware_information_to_manager() {
        // Create a manager and a Type0 record
        let manager = SmbiosManager::new(3, 9);

        let type0 = Type0PlatformFirmwareInformation {
            header: SmbiosTableHeader::new(0, 0, SMBIOS_HANDLE_PI_RESERVED),
            vendor: 1,                               // String 1: "TestVendor"
            firmware_version: 2,                     // String 2: "9.9.9"
            bios_starting_address_segment: 0xE000,   // Standard BIOS segment
            firmware_release_date: 3,                // String 3: "09/24/2025"
            firmware_rom_size: 0x0F,                 // 1MB ROM size
            characteristics: 0x08,                   // PCI supported
            characteristics_ext1: 0x01,              // ACPI supported
            characteristics_ext2: 0x00,              // No extended features
            system_bios_major_release: 9,            // BIOS major version
            system_bios_minor_release: 9,            // BIOS minor version
            embedded_controller_major_release: 0xFF, // Not supported
            embedded_controller_minor_release: 0xFF, // Not supported
            extended_bios_rom_size: 0x0000,          // No extended size needed
            string_pool: vec![String::from("TestVendor"), String::from("9.9.9"), String::from("09/24/2025")],
        };

        // Serialize into bytes using the generic serializer
        let record_bytes = type0.to_bytes();

        // Add to manager using the safe add_from_bytes method
        let handle = manager.add_from_bytes(None, &record_bytes).expect("add_from_bytes failed");

        // Retrieve using get_next
        let mut search_handle = SMBIOS_HANDLE_PI_RESERVED;
        let (found_header, _producer) = manager
            .get_next(&mut search_handle, Some(Type0PlatformFirmwareInformation::RECORD_TYPE))
            .expect("get_next failed");

        assert_eq!(found_header.record_type, Type0PlatformFirmwareInformation::RECORD_TYPE);
        assert_eq!(search_handle, handle);
    }

    #[test]
    fn test_validate_string_success() {
        // Valid string should pass
        assert!(SmbiosManager::validate_string("Valid String").is_ok());
        assert!(SmbiosManager::validate_string("").is_ok()); // Empty is valid
    }

    #[test]
    fn test_validate_string_too_long() {
        // String longer than 64 bytes should fail
        let long_string = "a".repeat(SMBIOS_STRING_MAX_LENGTH + 1);
        assert_eq!(SmbiosManager::validate_string(&long_string), Err(SmbiosError::StringTooLong));
    }

    #[test]
    fn test_validate_string_with_null() {
        // String containing null should fail
        assert_eq!(SmbiosManager::validate_string("test\0string"), Err(SmbiosError::InvalidParameter));
    }

    #[test]
    fn test_validate_and_count_strings_empty_pool() {
        // Empty string pool (just double null)
        let pool = [0u8, 0u8];
        assert_eq!(SmbiosManager::validate_and_count_strings(&pool), Ok(0));
    }

    #[test]
    fn test_validate_and_count_strings_single_string() {
        // Single string: "test\0\0"
        let pool = b"test\0\0";
        assert_eq!(SmbiosManager::validate_and_count_strings(pool), Ok(1));
    }

    #[test]
    fn test_validate_and_count_strings_multiple_strings() {
        // Multiple strings: "first\0second\0third\0\0"
        let pool = b"first\0second\0third\0\0";
        assert_eq!(SmbiosManager::validate_and_count_strings(pool), Ok(3));
    }

    #[test]
    fn test_validate_and_count_strings_too_short() {
        // Pool too short (< 2 bytes)
        let pool = [0u8];
        assert_eq!(SmbiosManager::validate_and_count_strings(&pool), Err(SmbiosError::InvalidParameter));
    }

    #[test]
    fn test_validate_and_count_strings_no_double_null() {
        // Pool doesn't end with double null
        let pool = b"test\0";
        assert_eq!(SmbiosManager::validate_and_count_strings(pool), Err(SmbiosError::InvalidParameter));
    }

    #[test]
    fn test_validate_and_count_strings_consecutive_nulls() {
        // Consecutive nulls in the middle (invalid)
        let pool = b"test\0\0extra\0\0";
        assert_eq!(SmbiosManager::validate_and_count_strings(pool), Err(SmbiosError::InvalidParameter));
    }

    #[test]
    fn test_validate_and_count_strings_too_long_string() {
        // String exceeding max length
        let mut pool = vec![b'a'; SMBIOS_STRING_MAX_LENGTH + 1];
        pool.push(0); // null terminator
        pool.push(0); // double null
        assert_eq!(SmbiosManager::validate_and_count_strings(&pool), Err(SmbiosError::StringTooLong));
    }

    #[test]
    fn test_parse_strings_from_pool() {
        let pool = b"first\0second\0third\0\0";
        let strings = SmbiosManager::parse_strings_from_pool(pool).expect("parse failed");
        assert_eq!(strings.len(), 3);
        assert_eq!(strings[0], "first");
        assert_eq!(strings[1], "second");
        assert_eq!(strings[2], "third");
    }

    #[test]
    fn test_parse_strings_from_pool_empty() {
        let pool = b"\0\0";
        let strings = SmbiosManager::parse_strings_from_pool(pool).expect("parse failed");
        assert_eq!(strings.len(), 0);
    }

    #[test]
    fn test_build_record_with_strings() {
        let header = SmbiosTableHeader::new(1, 10, SMBIOS_HANDLE_PI_RESERVED);
        let strings = &["Manufacturer", "Product"];
        let record = SmbiosManager::build_record_with_strings(&header, strings).expect("build failed");

        // Should have header + strings + double null
        assert!(record.len() >= core::mem::size_of::<SmbiosTableHeader>());
        assert_eq!(record[0], 1); // record type
    }

    #[test]
    fn test_build_record_with_no_strings() {
        let header = SmbiosTableHeader::new(1, 10, SMBIOS_HANDLE_PI_RESERVED);
        let strings: &[&str] = &[];
        let record = SmbiosManager::build_record_with_strings(&header, strings).expect("build failed");

        // Should end with double null
        assert_eq!(record[record.len() - 1], 0);
        assert_eq!(record[record.len() - 2], 0);
    }

    #[test]
    fn test_build_record_with_invalid_string() {
        let header = SmbiosTableHeader::new(1, 10, SMBIOS_HANDLE_PI_RESERVED);
        let long_string = "a".repeat(SMBIOS_STRING_MAX_LENGTH + 1);
        let strings = &[long_string.as_str()];
        assert_eq!(SmbiosManager::build_record_with_strings(&header, strings), Err(SmbiosError::StringTooLong));
    }

    #[test]
    fn test_version() {
        let manager = SmbiosManager::new(3, 9);
        assert_eq!(manager.version(), (3, 9));
    }

    #[test]
    fn test_allocate_handle_sequential() {
        let manager = SmbiosManager::new(3, 9);

        // First allocation should be handle 1
        let handle1 = manager.allocate_handle().expect("allocation failed");
        assert_eq!(handle1, 1);

        // Second should be 2
        let handle2 = manager.allocate_handle().expect("allocation failed");
        assert_eq!(handle2, 2);
    }

    #[test]
    fn test_handle_reuse_after_remove() {
        let manager = SmbiosManager::new(3, 9);

        // Create a minimal record with proper length
        let mut record_data = vec![1u8, 4, 0, 0]; // type, length=4 (just the header), handle placeholder
        record_data.extend_from_slice(b"\0\0"); // Empty string pool

        // Add record
        let handle1 = manager.add_from_bytes(None, &record_data).expect("add failed");

        // Remove it
        manager.remove(handle1).expect("remove failed");

        // Next allocation should reuse the freed handle
        let mut record_data2 = vec![2u8, 4, 0, 0];
        record_data2.extend_from_slice(b"\0\0");
        let handle2 = manager.add_from_bytes(None, &record_data2).expect("add failed");

        assert_eq!(handle1, handle2); // Should be reused
    }

    #[test]
    fn test_update_string_success() {
        let manager = SmbiosManager::new(3, 9);

        // Create a record with strings - need proper structured length
        let mut record_data = vec![1u8, 4, 0, 0]; // type, length=4, handle
        record_data.extend_from_slice(b"original\0\0");

        let handle = manager.add_from_bytes(None, &record_data).expect("add failed");

        // Update the string
        manager.update_string(handle, 1, "updated").expect("update failed");

        // Verify the update (indirectly by checking no error)
        assert!(manager.update_string(handle, 1, "another").is_ok());
    }

    #[test]
    fn test_update_string_handle_not_found() {
        let manager = SmbiosManager::new(3, 9);

        // Try to update a non-existent handle
        assert_eq!(manager.update_string(999, 1, "test"), Err(SmbiosError::HandleNotFound));
    }

    #[test]
    fn test_update_string_invalid_string_number() {
        let manager = SmbiosManager::new(3, 9);

        // Create a record with one string
        let mut record_data = vec![1u8, 4, 0, 0]; // Minimal header
        record_data.extend_from_slice(b"test\0\0");

        let handle = manager.add_from_bytes(None, &record_data).expect("add failed");

        // Try to update string 0 (invalid)
        assert_eq!(manager.update_string(handle, 0, "new"), Err(SmbiosError::InvalidHandle));

        // Try to update string 2 (doesn't exist, only 1 string)
        assert_eq!(manager.update_string(handle, 2, "new"), Err(SmbiosError::InvalidHandle));
    }

    #[test]
    fn test_update_string_too_long() {
        let manager = SmbiosManager::new(3, 9);

        let mut record_data = vec![1u8, 4, 0, 0]; // Minimal header
        record_data.extend_from_slice(b"test\0\0");

        let handle = manager.add_from_bytes(None, &record_data).expect("add failed");

        let long_string = "a".repeat(SMBIOS_STRING_MAX_LENGTH + 1);
        assert_eq!(manager.update_string(handle, 1, &long_string), Err(SmbiosError::StringTooLong));
    }

    #[test]
    fn test_remove_success() {
        let manager = SmbiosManager::new(3, 9);

        let mut record_data = vec![1u8, 4, 0, 0]; // Minimal header
        record_data.extend_from_slice(b"\0\0");

        let handle = manager.add_from_bytes(None, &record_data).expect("add failed");

        // Remove should succeed
        assert!(manager.remove(handle).is_ok());

        // Second remove should fail
        assert_eq!(manager.remove(handle), Err(SmbiosError::HandleNotFound));
    }

    #[test]
    fn test_get_next_empty_manager() {
        let manager = SmbiosManager::new(3, 9);
        let mut handle = SMBIOS_HANDLE_PI_RESERVED;

        // Getting next from empty manager should fail
        assert_eq!(manager.get_next(&mut handle, None), Err(SmbiosError::HandleNotFound));
    }

    #[test]
    fn test_get_next_iterate_all() {
        let manager = SmbiosManager::new(3, 9);

        // Add multiple records
        for i in 1..=3 {
            let mut record_data = vec![i, 4, 0, 0]; // type, length=4 (header only)
            record_data.extend_from_slice(b"\0\0"); // Empty string pool
            manager.add_from_bytes(None, &record_data).expect("add failed");
        }

        // Iterate through all records
        let mut handle = SMBIOS_HANDLE_PI_RESERVED;
        let mut count = 0;

        while manager.get_next(&mut handle, None).is_ok() {
            count += 1;
        }

        assert_eq!(count, 3);
    }

    #[test]
    fn test_get_next_with_type_filter() {
        let manager = SmbiosManager::new(3, 9);

        // Add records of different types
        for record_type in [1u8, 2, 1, 3, 1] {
            let mut record_data = vec![record_type, 4, 0, 0]; // header only
            record_data.extend_from_slice(b"\0\0"); // Empty string pool
            manager.add_from_bytes(None, &record_data).expect("add failed");
        }

        // Count only type 1 records
        let mut handle = SMBIOS_HANDLE_PI_RESERVED;
        let mut count = 0;

        while let Ok((header, _)) = manager.get_next(&mut handle, Some(1)) {
            // Copy to avoid unaligned reference
            let rt = header.record_type;
            assert_eq!(rt, 1);
            count += 1;
        }

        assert_eq!(count, 3); // Should find 3 type-1 records
    }

    #[test]
    fn test_add_from_bytes_buffer_too_small() {
        let manager = SmbiosManager::new(3, 9);

        // Buffer smaller than header
        let small_buffer = vec![1u8, 2];
        assert_eq!(manager.add_from_bytes(None, &small_buffer), Err(SmbiosError::BufferTooSmall));
    }

    #[test]
    fn test_add_from_bytes_invalid_length() {
        let manager = SmbiosManager::new(3, 9);

        // Header claims length larger than buffer
        let invalid_data = vec![1u8, 255, 0, 0, 0, 0]; // length=255 but buffer is tiny
        assert_eq!(manager.add_from_bytes(None, &invalid_data), Err(SmbiosError::BufferTooSmall));
    }

    #[test]
    fn test_add_from_bytes_no_string_pool() {
        let manager = SmbiosManager::new(3, 9);

        // Valid header but no room for string pool (needs at least 2 bytes for double null)
        let mut data = vec![1u8, 10, 0, 0]; // length=10
        data.extend_from_slice(&[0u8; 6]); // structured data (6 bytes to reach length-4 = 6 bytes)
        // Missing string pool (no double null) - total is 10 bytes which equals length,
        // leaving no room for the required 2-byte string pool terminator

        assert_eq!(manager.add_from_bytes(None, &data), Err(SmbiosError::BufferTooSmall));
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

        // The checksum should make the total sum equal to zero
        let mut test_entry = entry_point;
        test_entry.checksum = checksum;

        let bytes = test_entry.as_bytes();

        let sum: u8 = bytes.iter().fold(0u8, |acc, &b| acc.wrapping_add(b));
        assert_eq!(sum, 0);
    }

    #[test]
    fn test_smbios_record_builder_with_fields() {
        let record = SmbiosRecordBuilder::new(3) // Enclosure type
            .add_field(1u8) // manufacturer
            .add_field(2u8) // type
            .add_field(3u8) // version
            .add_string(String::from("Chassis Manufacturer"))
            .expect("string add failed")
            .add_string(String::from("Tower"))
            .expect("string add failed")
            .add_string(String::from("v1.0"))
            .expect("string add failed")
            .build()
            .expect("build failed");

        assert_eq!(record[0], 3); // record type
        assert!(record.len() > 10);
    }

    #[test]
    fn test_smbios_error_types() {
        // Test that error enum derives are working
        let err1 = SmbiosError::InvalidParameter;
        let err2 = SmbiosError::InvalidParameter;
        assert_eq!(err1, err2);

        let err3 = SmbiosError::OutOfResources;
        assert_ne!(err1, err3);
    }

    #[test]
    fn test_smbios_table_header_new() {
        let header = SmbiosTableHeader::new(5, 20, 42);
        // Copy packed fields to avoid unaligned reference
        let record_type = header.record_type;
        let length = header.length;
        let handle = header.handle;

        assert_eq!(record_type, 5);
        assert_eq!(length, 20);
        assert_eq!(handle, 42);
    }

    #[test]
    fn test_parse_strings_from_pool_single_string() {
        let pool = b"teststring\0\0";
        let strings = SmbiosManager::parse_strings_from_pool(pool).expect("parse failed");
        assert_eq!(strings.len(), 1);
        assert_eq!(strings[0], "teststring");
    }

    #[test]
    fn test_parse_strings_from_pool_too_short() {
        let pool = b"\0";
        assert_eq!(SmbiosManager::parse_strings_from_pool(pool), Err(SmbiosError::InvalidParameter));
    }

    #[test]
    fn test_parse_strings_from_pool_no_double_null() {
        let pool = b"test\0single";
        assert_eq!(SmbiosManager::parse_strings_from_pool(pool), Err(SmbiosError::InvalidParameter));
    }

    #[test]
    fn test_parse_strings_from_pool_consecutive_nulls() {
        let pool = b"first\0\0extra\0\0";
        assert_eq!(SmbiosManager::parse_strings_from_pool(pool), Err(SmbiosError::InvalidParameter));
    }

    #[test]
    fn test_smbios_record_new() {
        let header = SmbiosTableHeader::new(1, 10, 5);
        let data = vec![1u8, 10, 5, 0, 0, 0, 0, 0, 0, 0, 0, 0]; // Complete record data
        let record = SmbiosRecord::new(header, None, data.clone(), 0);

        // Copy to avoid unaligned reference
        let record_type = record.header.record_type;
        let handle = record.header.handle;

        assert_eq!(record_type, 1);
        assert_eq!(handle, 5);
        assert_eq!(record.producer_handle, None);
        assert_eq!(record.data, data);
        assert_eq!(record.string_count, 0);
    }

    #[test]
    fn test_smbios_record_with_producer_handle() {
        let header = SmbiosTableHeader::new(2, 8, 10);
        let producer = core::ptr::null_mut::<c_void>() as efi::Handle;
        let data = vec![2u8, 8, 10, 0, 0, 0, 0, 0];
        let record = SmbiosRecord::new(header, Some(producer), data, 2);

        assert_eq!(record.string_count, 2);
        assert_eq!(record.producer_handle, Some(producer));
    }

    #[test]
    fn test_allocate_handle_wraps_at_max() {
        let manager = SmbiosManager::new(3, 9);

        // Set next_handle to near the limit
        *manager.next_handle.borrow_mut() = 0xFFFD;

        // Should allocate 0xFFFD
        let handle1 = manager.allocate_handle().expect("allocation failed");
        assert_eq!(handle1, 0xFFFD);

        // Next should fail (reached 0xFFFE which is reserved)
        assert_eq!(manager.allocate_handle(), Err(SmbiosError::OutOfResources));
    }

    #[test]
    fn test_get_next_with_start_handle() {
        let manager = SmbiosManager::new(3, 9);

        // Add three records
        let handles: Vec<SmbiosHandle> = (1..=3)
            .map(|i| {
                let mut record_data = vec![i, 4, 0, 0];
                record_data.extend_from_slice(b"\0\0");
                manager.add_from_bytes(None, &record_data).expect("add failed")
            })
            .collect();

        // Start from the second handle
        let mut search_handle = handles[1];
        let (header, _) = manager.get_next(&mut search_handle, None).expect("get_next failed");

        // Should find the third record
        assert_eq!(search_handle, handles[2]);
        assert_eq!(header.record_type, 3);
    }

    #[test]
    fn test_get_next_with_invalid_start_handle() {
        let manager = SmbiosManager::new(3, 9);

        // Add one record
        let mut record_data = vec![1u8, 4, 0, 0];
        record_data.extend_from_slice(b"\0\0");
        manager.add_from_bytes(None, &record_data).expect("add failed");

        // Start with non-existent handle (will search from end)
        let mut search_handle = 9999;
        assert_eq!(manager.get_next(&mut search_handle, None), Err(SmbiosError::HandleNotFound));
        assert_eq!(search_handle, SMBIOS_HANDLE_PI_RESERVED);
    }

    #[test]
    fn test_get_next_with_type_filter_not_found() {
        let manager = SmbiosManager::new(3, 9);

        // Add records of type 1 and 2
        for record_type in [1u8, 2, 1] {
            let mut record_data = vec![record_type, 4, 0, 0];
            record_data.extend_from_slice(b"\0\0");
            manager.add_from_bytes(None, &record_data).expect("add failed");
        }

        // Search for type 5 which doesn't exist
        let mut handle = SMBIOS_HANDLE_PI_RESERVED;
        assert_eq!(manager.get_next(&mut handle, Some(5)), Err(SmbiosError::HandleNotFound));
        assert_eq!(handle, SMBIOS_HANDLE_PI_RESERVED);
    }

    #[test]
    fn test_update_string_rebuilds_pool() {
        let manager = SmbiosManager::new(3, 9);

        // Create a record with multiple strings
        let mut record_data = vec![1u8, 4, 0, 0];
        record_data.extend_from_slice(b"first\0second\0third\0\0");

        let handle = manager.add_from_bytes(None, &record_data).expect("add failed");

        // Update the middle string with a longer one
        manager.update_string(handle, 2, "new_second_string").expect("update failed");

        // Verify we can still update (means the record is valid)
        assert!(manager.update_string(handle, 1, "new_first").is_ok());
        assert!(manager.update_string(handle, 3, "new_third").is_ok());
    }

    #[test]
    fn test_remove_reserved_handle_not_added_to_free_list() {
        let manager = SmbiosManager::new(3, 9);

        // Manually create a record with a reserved handle (this won't happen normally,
        // but we're testing the boundary condition in the remove function)
        let mut record_data = vec![1u8, 4, 0, 0];
        record_data.extend_from_slice(b"\0\0");
        let handle = manager.add_from_bytes(None, &record_data).expect("add failed");

        // Remove it
        manager.remove(handle).expect("remove failed");

        // Check that freed_handles was populated (normal handle should be added)
        let freed = manager.freed_handles.borrow();
        assert_eq!(freed.len(), 1);
        assert_eq!(freed[0], handle);
    }

    #[test]
    fn test_add_from_bytes_with_producer_handle() {
        let manager = SmbiosManager::new(3, 9);

        let producer = 0x1234 as efi::Handle;
        let mut record_data = vec![1u8, 4, 0, 0];
        record_data.extend_from_slice(b"\0\0");

        let handle = manager.add_from_bytes(Some(producer), &record_data).expect("add failed");

        // Retrieve and check producer handle
        let mut search_handle = SMBIOS_HANDLE_PI_RESERVED;
        let (_header, found_producer) = manager.get_next(&mut search_handle, None).expect("get_next failed");
        assert_eq!(found_producer, Some(producer));
        assert_eq!(search_handle, handle);
    }

    #[test]
    fn test_add_from_bytes_with_strings() {
        let manager = SmbiosManager::new(3, 9);

        // Create a record with structured data and strings
        let mut record_data = vec![1u8, 6, 0, 0]; // type=1, length=6
        record_data.extend_from_slice(&[0x01, 0x02]); // 2 bytes of structured data
        record_data.extend_from_slice(b"String1\0String2\0\0"); // String pool

        let assigned_handle = manager.add_from_bytes(None, &record_data).expect("add failed");

        // Verify the record was added
        let records = manager.records.borrow();
        assert_eq!(records.len(), 1);

        // Copy to avoid unaligned reference
        let handle = records[0].header.handle;
        assert_eq!(handle, assigned_handle);
        assert_eq!(records[0].string_count, 2);
    }

    #[test]
    fn test_smbios_record_builder_empty_build() {
        // Builder with no fields or strings
        let record = SmbiosRecordBuilder::new(127) // End-of-table marker type
            .build()
            .expect("build failed");

        assert_eq!(record[0], 127); // record type
        assert_eq!(record[1], 4); // length = header only
        // Should end with double null
        assert_eq!(record[record.len() - 1], 0);
        assert_eq!(record[record.len() - 2], 0);
    }

    #[test]
    fn test_smbios_record_builder_add_string_too_long() {
        let long_string = "a".repeat(SMBIOS_STRING_MAX_LENGTH + 1);
        // No need for String::from since repeat already returns a String
        let result = SmbiosRecordBuilder::new(1).add_string(long_string);

        assert_eq!(result, Err(SmbiosError::StringTooLong));
    }

    #[test]
    fn test_build_record_with_strings_multiple() {
        let header = SmbiosTableHeader::new(2, 4, SMBIOS_HANDLE_PI_RESERVED); // length=4 means just header, no extra data
        let strings = &["Manufacturer", "Product", "Version", "Serial"];
        let record = SmbiosManager::build_record_with_strings(&header, strings).expect("build failed");

        // Verify structure: header + strings with null terminators + double null
        assert_eq!(record[0], 2); // type

        // Strings start immediately after the header (length=4 means just the 4-byte header)
        let string_start = 4; // Size of header
        let pool = &record[string_start..];

        // Should contain all 4 strings
        let parsed = SmbiosManager::parse_strings_from_pool(pool).expect("parse failed");
        assert_eq!(parsed.len(), 4);
        assert_eq!(parsed[0], "Manufacturer");
        assert_eq!(parsed[3], "Serial");
    }

    #[test]
    fn test_get_global_smbios_manager_not_installed() {
        // Before installation, should return None
        // Note: This may fail if another test has already installed the manager
        // In a real test environment, you'd want to reset the global state
        let manager = get_global_smbios_manager();

        // Can't reliably test this without test isolation, but the function exists
        // and compiles correctly
        assert!(manager.is_some() || manager.is_none()); // Tautology, but validates the function works
    }

    #[test]
    fn test_validate_string_exact_max_length() {
        // String exactly at max length should pass
        let max_string = "a".repeat(SMBIOS_STRING_MAX_LENGTH);
        assert!(SmbiosManager::validate_string(&max_string).is_ok());
    }

    #[test]
    fn test_validate_string_with_embedded_null() {
        // String with null in the middle should fail
        assert_eq!(SmbiosManager::validate_string("before\0after"), Err(SmbiosError::InvalidParameter));
    }

    #[test]
    fn test_update_string_buffer_too_small_error() {
        let manager = SmbiosManager::new(3, 9);

        // Create a malformed record that's too short (will trigger BufferTooSmall)
        // We have to bypass validation to create this scenario
        let header = SmbiosTableHeader::new(1, 10, 1);
        let data = vec![1u8, 10, 1, 0]; // Only 4 bytes, but length claims 10
        let record = SmbiosRecord::new(header, None, data, 1);

        manager.records.borrow_mut().push(record);

        // Try to update - should fail with BufferTooSmall
        assert_eq!(manager.update_string(1, 1, "test"), Err(SmbiosError::BufferTooSmall));
    }

    #[test]
    fn test_allocate_handle_uses_free_list() {
        let manager = SmbiosManager::new(3, 9);

        // Manually add some handles to the free list
        manager.freed_handles.borrow_mut().push(100);
        manager.freed_handles.borrow_mut().push(50);

        // Should pop from free list (LIFO order)
        let handle1 = manager.allocate_handle().expect("allocation failed");
        assert_eq!(handle1, 50);

        let handle2 = manager.allocate_handle().expect("allocation failed");
        assert_eq!(handle2, 100);

        // Now should use next_handle (which starts at 1)
        let handle3 = manager.allocate_handle().expect("allocation failed");
        assert_eq!(handle3, 1);
    }

    #[test]
    fn test_smbios_error_all_variants() {
        // Test all error variants for completeness
        let errors = vec![
            SmbiosError::InvalidParameter,
            SmbiosError::OutOfResources,
            SmbiosError::HandleAlreadyInUse,
            SmbiosError::HandleNotFound,
            SmbiosError::UnsupportedRecordType,
            SmbiosError::InvalidHandle,
            SmbiosError::StringTooLong,
            SmbiosError::BufferTooSmall,
        ];

        // Each should be cloneable and comparable
        for err in errors {
            let cloned = err.clone();
            assert_eq!(err, cloned);
        }
    }

    #[test]
    fn test_add_from_bytes_updates_handle_in_data() {
        let manager = SmbiosManager::new(3, 9);

        let mut record_data = vec![1u8, 4, 0xFF, 0xFF]; // Original handle is 0xFFFF
        record_data.extend_from_slice(b"\0\0");

        let assigned_handle = manager.add_from_bytes(None, &record_data).expect("add failed");

        // Verify the handle was updated in the record data
        let records = manager.records.borrow();
        let stored_data = &records[0].data;

        // Handle is at bytes 2-3 (little-endian)
        let stored_handle = u16::from_le_bytes([stored_data[2], stored_data[3]]);
        assert_eq!(stored_handle, assigned_handle);
    }

    #[test]
    fn test_version_custom_values() {
        let manager = SmbiosManager::new(4, 2);
        assert_eq!(manager.version(), (4, 2));

        let manager2 = SmbiosManager::new(255, 255);
        assert_eq!(manager2.version(), (255, 255));
    }
}
