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
use core::cell::RefCell;
use core::ffi::{c_char, c_void};
use core::sync::atomic::{AtomicPtr, Ordering};
use patina::boot_services::BootServices;
use patina::uefi_protocol::ProtocolInterface;
use patina::uefi_size_to_pages;
use r_efi::efi;
use r_efi::efi::Handle;
use r_efi::efi::PhysicalAddress;
use spin::Mutex;
use zerocopy_derive::{FromBytes, Immutable, IntoBytes, KnownLayout};

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

/// Enhanced error handling
#[derive(Debug, Clone, PartialEq)]
pub enum SmbiosError {
    InvalidParameter,
    OutOfResources,
    HandleAlreadyInUse,
    HandleNotFound,
    UnsupportedRecordType,
    InvalidHandle,
    StringTooLong,
    BufferTooSmall,
}

pub trait SmbiosRecords<'a> {
    // Note: The unsafe `add` method has been removed. It was only needed for C protocol
    // compatibility, but that use case is now handled by the efiapi wrapper which converts
    // the C pointer to a byte slice and calls `add_from_bytes` directly.

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
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use patina_smbios::smbios_record::Type0PlatformFirmwareInformation;
    ///
    /// let mut bios_info = Type0PlatformFirmwareInformation::new();
    /// bios_info.string_pool = vec![
    ///     "Vendor Name".to_string(),
    ///     "Version 1.0".to_string(),
    ///     "01/01/2025".to_string(),
    /// ];
    ///
    /// let record_bytes = bios_info.to_bytes();
    /// let handle = smbios_records.add_from_bytes(None, &record_bytes)?;
    /// ```
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

/// SMBIOS 3.0 entry point structure (64-bit)
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct Smbios30EntryPoint {
    pub anchor_string: [u8; 5], // "_SM3_"
    pub checksum: u8,
    pub length: u8,
    pub major_version: u8,
    pub minor_version: u8,
    pub doc_rev: u8,
    pub revision: u8,
    pub reserved: u8,
    pub table_max_size: u32,
    pub table_address: u64,
}

pub struct SmbiosManager {
    records: RefCell<Vec<SmbiosRecord>>,
    next_handle: RefCell<SmbiosHandle>,
    freed_handles: RefCell<Vec<SmbiosHandle>>,
    major_version: u8,
    minor_version: u8,
    entry_point_64: RefCell<Option<Box<Smbios30EntryPoint>>>,
    table_64_address: RefCell<Option<PhysicalAddress>>,
    lock: Mutex<()>,
}

impl Clone for SmbiosManager {
    fn clone(&self) -> Self {
        Self {
            records: RefCell::new(self.records.borrow().clone()),
            next_handle: RefCell::new(*self.next_handle.borrow()),
            freed_handles: RefCell::new(self.freed_handles.borrow().clone()),
            major_version: self.major_version,
            minor_version: self.minor_version,
            entry_point_64: RefCell::new(self.entry_point_64.borrow().as_ref().map(|ep| Box::new(**ep))),
            table_64_address: RefCell::new(*self.table_64_address.borrow()),
            lock: Mutex::new(()),
        }
    }
}

impl SmbiosManager {
    pub fn new(major_version: u8, minor_version: u8) -> Self {
        Self {
            records: RefCell::new(Vec::new()),
            next_handle: RefCell::new(1),
            freed_handles: RefCell::new(Vec::new()),
            major_version,
            minor_version,
            entry_point_64: RefCell::new(None),
            table_64_address: RefCell::new(None),
            lock: Mutex::new(()),
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
    /// # Examples
    /// ```ignore
    /// // Three strings: "Patina\0Firmware\0v1.0\0\0"
    /// let pool = b"Patina\0Firmware\0v1.0\0\0";
    /// let count = validate_and_count_strings(pool)?; // Returns 3
    ///
    /// // Empty pool: "\0\0"
    /// let empty = b"\0\0";
    /// let count = validate_and_count_strings(empty)?; // Returns 0
    ///
    /// // Invalid pool (single null): "\0"
    /// let invalid = b"\0";
    /// validate_and_count_strings(invalid); // Returns Err(InvalidParameter)
    /// ```
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
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let header = SmbiosTableHeader::new(1, size_of::<MyStruct>(), SMBIOS_HANDLE_PI_RESERVED);
    /// let strings = &["Manufacturer", "Product Name", "Version"];
    /// let record = SmbiosManager::build_record_with_strings(&header, strings)?;
    /// manager.add_from_bytes(None, &record)?;
    /// ```
    #[allow(dead_code)]
    pub fn build_record_with_strings(header: &SmbiosTableHeader, strings: &[&str]) -> Result<Vec<u8>, SmbiosError> {
        // Validate all strings first
        for s in strings {
            Self::validate_string(s)?;
        }

        let mut record = Vec::new();

        // Add the structured data using zerocopy
        use zerocopy::IntoBytes;
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
        use patina::boot_services::allocation::{AllocType, MemoryType};

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
        unsafe {
            let table_slice = core::slice::from_raw_parts_mut(table_address as *mut u8, total_table_size);
            let mut offset = 0;

            for record in records.iter() {
                let record_bytes = record.data.as_slice();
                table_slice[offset..offset + record_bytes.len()].copy_from_slice(record_bytes);
                offset += record_bytes.len();
            }
        }

        // Step 4: Create SMBIOS 3.0 Entry Point Structure
        let mut entry_point = Smbios30EntryPoint {
            anchor_string: *b"_SM3_",
            checksum: 0, // Will be calculated
            length: core::mem::size_of::<Smbios30EntryPoint>() as u8,
            major_version: self.major_version,
            minor_version: self.minor_version,
            doc_rev: 0,  // SMBIOS 3.0 document revision
            revision: 1, // Entry point structure revision (0x01 for 3.0)
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
        unsafe {
            let ep_ptr = ep_address as *mut Smbios30EntryPoint;
            core::ptr::write(ep_ptr, entry_point);
        }

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
    /// Uses unsafe code to reinterpret the entry point structure as a byte slice
    /// for checksum calculation. This is safe because the structure is repr(C, packed).
    fn calculate_checksum(entry_point: &Smbios30EntryPoint) -> u8 {
        let bytes = unsafe {
            core::slice::from_raw_parts(
                entry_point as *const _ as *const u8,
                core::mem::size_of::<Smbios30EntryPoint>(),
            )
        };

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
        use zerocopy::Ref;
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
        let _lock = self.lock.lock();

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
        let _lock = self.lock.lock();

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
        let _lock = self.lock.lock();
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
#[derive(Debug, Clone, FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct SmbiosTableHeader {
    pub record_type: SmbiosType,
    pub length: u8,
    pub handle: SmbiosHandle,
}

impl SmbiosTableHeader {
    pub fn new(record_type: SmbiosType, length: u8, handle: SmbiosHandle) -> Self {
        Self { record_type, length, handle }
    }
}

/// Internal SMBIOS record representation
///
/// This implementation is for SMBIOS 3.0+ specification which uses 64-bit addressing.
#[derive(Clone)]
pub struct SmbiosRecord {
    pub header: SmbiosTableHeader,
    pub producer_handle: Option<Handle>,
    pub data: Vec<u8>, // Complete record including strings
    string_count: usize,
}

impl SmbiosRecord {
    pub fn new(header: SmbiosTableHeader, producer_handle: Option<Handle>, data: Vec<u8>, string_count: usize) -> Self {
        Self { header, producer_handle, data, string_count }
    }
}

pub struct SmbiosRecordBuilder {
    record_type: u8,
    data: Vec<u8>,
    strings: Vec<String>,
}

impl SmbiosRecordBuilder {
    pub fn new(record_type: u8) -> Self {
        Self { record_type, data: Vec::new(), strings: Vec::new() }
    }

    pub fn add_field<T>(mut self, value: T) -> Self
    where
        T: Copy + zerocopy::IntoBytes + zerocopy::Immutable,
    {
        self.data.extend_from_slice(value.as_bytes());
        self
    }

    pub fn add_string(mut self, s: String) -> Result<Self, SmbiosError> {
        SmbiosManager::validate_string(&s)?;
        self.strings.push(s);
        Ok(self)
    }

    pub fn build(self) -> Result<Vec<u8>, SmbiosError> {
        let mut record = Vec::new();

        // Add header using zerocopy
        let header = SmbiosTableHeader {
            record_type: self.record_type,
            length: (core::mem::size_of::<SmbiosTableHeader>() + self.data.len()) as u8,
            handle: SMBIOS_HANDLE_PI_RESERVED,
        };

        use zerocopy::IntoBytes;
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

/// Global storage for the SMBIOS manager instance that the C protocol will use
///
/// # Safety
///
/// This is safe because:
/// - UEFI runs in a single-threaded environment during DXE phase
/// - The pointer is only set once during component initialization
/// - The manager has 'static lifetime (leaked Box)
/// - Access is protected by the Mutex inside SmbiosManager
static SMBIOS_MANAGER: AtomicPtr<Mutex<SmbiosManager>> = AtomicPtr::new(core::ptr::null_mut());

/// Storage for the protocol interface pointer (for lifetime management)
static SMBIOS_PROTOCOL_INTERFACE: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());

/// Storage for the protocol handle
static SMBIOS_PROTOCOL_HANDLE: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());

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

        // Get the global manager
        let manager_ptr = SMBIOS_MANAGER.load(Ordering::SeqCst);
        if manager_ptr.is_null() {
            return efi::Status::NOT_READY;
        }

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

            // SAFETY: manager_ptr is guaranteed to be valid (checked above)
            let manager = &*manager_ptr;
            let manager_lock = manager.lock();

            // Convert handle
            let producer_opt = if producer_handle.is_null() { None } else { Some(producer_handle) };

            // Add the record
            match manager_lock.add_from_bytes(producer_opt, full_record_bytes) {
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

        let manager_ptr = SMBIOS_MANAGER.load(Ordering::SeqCst);
        if manager_ptr.is_null() {
            return efi::Status::NOT_READY;
        }

        unsafe {
            let handle = *smbios_handle;
            let str_num = *string_number;

            // Convert C string to Rust str
            let c_str = core::ffi::CStr::from_ptr(string);
            let rust_str = match c_str.to_str() {
                Ok(s) => s,
                Err(_) => return efi::Status::INVALID_PARAMETER,
            };

            // SAFETY: manager_ptr is guaranteed to be valid
            let manager = &*manager_ptr;
            let manager_lock = manager.lock();

            match manager_lock.update_string(handle, str_num, rust_str) {
                Ok(()) => efi::Status::SUCCESS,
                Err(SmbiosError::InvalidParameter) => efi::Status::INVALID_PARAMETER,
                Err(SmbiosError::HandleNotFound) => efi::Status::NOT_FOUND,
                Err(SmbiosError::StringTooLong) => efi::Status::INVALID_PARAMETER,
                Err(_) => efi::Status::DEVICE_ERROR,
            }
        }
    }

    #[allow(dead_code)]
    extern "efiapi" fn remove_ext(_protocol: *const SmbiosProtocol, smbios_handle: SmbiosHandle) -> efi::Status {
        let manager_ptr = SMBIOS_MANAGER.load(Ordering::SeqCst);
        if manager_ptr.is_null() {
            return efi::Status::NOT_READY;
        }

        unsafe {
            // SAFETY: manager_ptr is guaranteed to be valid
            let manager = &*manager_ptr;
            let manager_lock = manager.lock();

            match manager_lock.remove(smbios_handle) {
                Ok(()) => efi::Status::SUCCESS,
                Err(SmbiosError::HandleNotFound) => efi::Status::NOT_FOUND,
                Err(_) => efi::Status::DEVICE_ERROR,
            }
        }
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

        let manager_ptr = SMBIOS_MANAGER.load(Ordering::SeqCst);
        if manager_ptr.is_null() {
            return efi::Status::NOT_READY;
        }

        unsafe {
            let mut handle = *smbios_handle;
            let type_filter = if record_type.is_null() { None } else { Some(*record_type) };

            // SAFETY: manager_ptr is guaranteed to be valid
            let manager = &*manager_ptr;
            let manager_lock = manager.lock();

            match manager_lock.get_next(&mut handle, type_filter) {
                Ok((header_value, prod_handle)) => {
                    *smbios_handle = handle;
                    // Allocate the header on the heap and return a pointer to it
                    // Note: This leaks memory, but matches the expected C API behavior
                    *record = Box::into_raw(Box::new(header_value));
                    if !producer_handle.is_null() {
                        *producer_handle = prod_handle.unwrap_or(core::ptr::null_mut());
                    }
                    efi::Status::SUCCESS
                }
                Err(SmbiosError::HandleNotFound) => efi::Status::NOT_FOUND,
                Err(_) => efi::Status::DEVICE_ERROR,
            }
        }
    }
}

/// Installs the SMBIOS protocol for C/EDKII driver compatibility
///
/// This function should be called after the SMBIOS service is registered.
/// It creates a C-compatible protocol interface that wraps the Rust manager.
///
/// # Arguments
///
/// * `manager` - A reference to the SmbiosManager that will handle protocol calls
/// * `boot_services` - The UEFI boot services for protocol installation
///
/// # Safety
///
/// This function leaks the manager reference (Box::leak) to ensure it has 'static lifetime.
/// The protocol will remain installed for the lifetime of the system.
pub fn install_smbios_protocol(
    manager: &SmbiosManager,
    boot_services: &impl patina::boot_services::BootServices,
) -> Result<efi::Handle, SmbiosError> {
    // Clone the manager and wrap it in a Mutex for thread-safe access
    let manager_clone = manager.clone();
    let manager_mutex = Box::new(Mutex::new(manager_clone));

    // Leak the mutex to get a 'static reference
    let manager_ptr = Box::into_raw(manager_mutex);

    // Store the manager pointer globally
    SMBIOS_MANAGER.store(manager_ptr, Ordering::SeqCst);

    // Get the version from the manager
    let (major, minor) = unsafe { (*manager_ptr).lock().version() };

    // Create the protocol instance
    let protocol = SmbiosProtocol::new(major, minor);
    let interface = Box::into_raw(Box::new(protocol));
    let interface_void = interface as *mut c_void;

    // Store the interface pointer for lifetime management
    SMBIOS_PROTOCOL_INTERFACE.store(interface_void, Ordering::SeqCst);

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
            SMBIOS_PROTOCOL_HANDLE.store(h, Ordering::SeqCst);
            Ok(h)
        }
        Err(status) => {
            // Clean up on failure
            unsafe {
                let _ = Box::from_raw(interface);
                let _ = Box::from_raw(manager_ptr);
            }
            SMBIOS_MANAGER.store(core::ptr::null_mut(), Ordering::SeqCst);
            SMBIOS_PROTOCOL_INTERFACE.store(core::ptr::null_mut(), Ordering::SeqCst);
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
        let mut manager = SmbiosManager::new(3, 8);

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
}
