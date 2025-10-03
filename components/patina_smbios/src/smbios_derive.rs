extern crate alloc;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::c_char;
use patina_sdk::uefi_protocol::ProtocolInterface;
use r_efi::efi;
use r_efi::efi::Handle;
use r_efi::efi::PhysicalAddress;
use spin::Mutex;

pub type SmbiosHandle = u16;

/// Special handle value for automatic assignment
pub const SMBIOS_HANDLE_PI_RESERVED: SmbiosHandle = 0xFFFE;

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
    /// Adds an SMBIOS record to the SMBIOS table.
    ///
    /// # Safety
    ///
    /// **WARNING: This method is unsafe and should be avoided in favor of `add_from_bytes`.**
    ///
    /// This method assumes that `record` points to a complete SMBIOS record structure
    /// where the header is followed by the record data. Incorrect usage can lead to:
    /// - Memory corruption
    /// - Security vulnerabilities
    /// - System instability
    /// - Non-compliant SMBIOS structures
    ///
    /// The caller must ensure that `record` points to a valid, complete SMBIOS record
    /// with at least `record.length` bytes of valid memory following the header.
    ///
    /// # Recommendation
    ///
    /// **Use `add_from_bytes` instead** - it provides the same functionality with better
    /// memory safety and specification compliance guarantees.
    ///
    /// # Returns
    ///
    /// Returns the assigned SMBIOS handle for the newly added record.
    unsafe fn add(
        &mut self,
        producer_handle: Option<Handle>,
        record: &SmbiosTableHeader,
    ) -> Result<SmbiosHandle, SmbiosError>;

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
    fn add_from_bytes(
        &mut self,
        producer_handle: Option<Handle>,
        record_data: &[u8],
    ) -> Result<SmbiosHandle, SmbiosError>;

    /// Updates a string in an existing SMBIOS record.
    fn update_string(
        &mut self,
        smbios_handle: SmbiosHandle,
        string_number: usize,
        string: &str,
    ) -> Result<(), SmbiosError>;

    /// Removes an SMBIOS record from the SMBIOS table.
    fn remove(&mut self, smbios_handle: SmbiosHandle) -> Result<(), SmbiosError>;

    /// Discovers SMBIOS records, optionally filtered by type.
    fn get_next(
        &self,
        smbios_handle: &mut SmbiosHandle,
        record_type: Option<SmbiosType>,
    ) -> Result<(&SmbiosTableHeader, Option<Handle>), SmbiosError>;

    /// Provides an iterator over all SMBIOS records.
    fn iter(&self) -> Box<dyn Iterator<Item = &'a SmbiosRecord> + 'a>;

    /// Gets the SMBIOS version information.
    fn version(&self) -> (u8, u8); // (major, minor)
}

/// SMBIOS entry point structures
#[repr(C, packed)]
pub struct SmbiosEntryPoint {
    pub anchor_string: [u8; 4], // "_SM_"
    pub checksum: u8,
    pub length: u8,
    pub major_version: u8,
    pub minor_version: u8,
    pub max_structure_size: u16,
    pub revision: u8,
    pub formatted_area: [u8; 5],
    pub intermediate_anchor: [u8; 5], // "_DMI_"
    pub intermediate_checksum: u8,
    pub table_length: u16,
    pub table_address: u32,
    pub structure_count: u16,
    pub bcd_revision: u8,
}

#[repr(C, packed)]
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
    records: Vec<SmbiosRecord>,
    next_handle: SmbiosHandle,
    major_version: u8,
    minor_version: u8,
    #[allow(dead_code)]
    entry_point_32: Option<Box<SmbiosEntryPoint>>,
    #[allow(dead_code)]
    entry_point_64: Option<Box<Smbios30EntryPoint>>,
    #[allow(dead_code)]
    table_32_address: Option<PhysicalAddress>,
    #[allow(dead_code)]
    table_64_address: Option<PhysicalAddress>,
    lock: Mutex<()>,
}

impl SmbiosManager {
    pub fn new(major_version: u8, minor_version: u8) -> Self {
        Self {
            records: Vec::new(),
            next_handle: 1,
            major_version,
            minor_version,
            entry_point_32: None,
            entry_point_64: None,
            table_32_address: None,
            table_64_address: None,
            lock: Mutex::new(()),
        }
    }

    fn validate_string(s: &str) -> Result<(), SmbiosError> {
        if s.len() > SMBIOS_STRING_MAX_LENGTH {
            return Err(SmbiosError::StringTooLong);
        }
        if s.contains('\0') {
            return Err(SmbiosError::InvalidParameter);
        }
        Ok(())
    }

    /// Count strings in a complete SMBIOS record data array
    ///
    /// This method parses the string pool section of an SMBIOS record and counts
    /// the number of valid strings. The string pool begins after the structured
    /// data (at offset header.length) and ends with a double null terminator.
    ///
    /// # Arguments
    /// * `record_data` - Complete SMBIOS record including header, structured data, and strings
    ///
    /// # Returns
    /// * `Ok(count)` - Number of non-empty strings found
    /// * `Err(SmbiosError)` - If the record format is invalid
    ///
    /// # Examples
    /// ```ignore
    /// // Record with 3 strings: "Vendor", "Version", "Date"
    /// // Byte layout: [header][structured_data]["Vendor\0Version\0Date\0\0"]
    /// let count = SmbiosManager::count_strings_in_record(&record_data)?;
    /// assert_eq!(count, 3);
    ///
    /// // Record with no strings: [header][structured_data]["\0\0"]
    /// let count = SmbiosManager::count_strings_in_record(&empty_record)?;
    /// assert_eq!(count, 0);
    /// ```
    fn count_strings_in_record(record_data: &[u8]) -> Result<usize, SmbiosError> {
        // Validate minimum record size (4-byte header minimum)
        if record_data.len() < 4 {
            return Err(SmbiosError::BufferTooSmall);
        }

        // Extract header length from byte 1 (0-indexed)
        let header_length = record_data[1] as usize;

        // Validate that record has at least the claimed header length
        if record_data.len() < header_length {
            return Err(SmbiosError::BufferTooSmall);
        }

        // String pool starts after the structured data
        let string_pool_start = header_length;

        // Validate that there's space for at least the required double-null terminator
        if record_data.len() < string_pool_start + 2 {
            return Err(SmbiosError::InvalidParameter);
        }

        // Extract string pool section
        let string_pool = &record_data[string_pool_start..];

        // Delegate to the specialized string pool validator/counter
        Self::validate_and_count_strings(string_pool)
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

        let mut count = 0;
        let mut i = 0;
        let data_end = len - 2; // Exclude the final double-null

        while i < data_end {
            let start = i;

            // Find the next null terminator (end of current string)
            while i < data_end && string_pool_area[i] != 0 {
                i += 1;
            }

            // If we found content before the null, it's a valid string
            if i > start {
                // Validate string length doesn't exceed SMBIOS spec limit
                let string_len = i - start;
                if string_len > SMBIOS_STRING_MAX_LENGTH {
                    return Err(SmbiosError::StringTooLong);
                }
                count += 1;
            } else if i == start {
                // Found null at start position = consecutive nulls (invalid)
                return Err(SmbiosError::InvalidParameter);
            }

            // Move past the null terminator
            i += 1;
        }

        Ok(count)
    }

    #[allow(dead_code)]
    fn build_record_with_strings(header: &SmbiosTableHeader, strings: &[&str]) -> Result<Vec<u8>, SmbiosError> {
        // Validate all strings first
        for s in strings {
            Self::validate_string(s)?;
        }

        let mut record = Vec::new();

        // Add the structured data
        let header_bytes =
            unsafe { core::slice::from_raw_parts(header as *const _ as *const u8, header.length as usize) };
        record.extend_from_slice(header_bytes);

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

    /// Check if a handle is already allocated by scanning the records vector
    fn is_handle_allocated(&self, handle: SmbiosHandle) -> bool {
        self.records.iter().any(|r| r.header.handle == handle)
    }

    fn allocate_handle(&mut self) -> Result<SmbiosHandle, SmbiosError> {
        // Counter-based allocation with wraparound per RFC:
        // Start from next_handle, try sequential allocation up to 0xFEFF
        // If all are allocated, wrap around from 1
        let mut attempts = 0u32;
        const MAX_ATTEMPTS: u32 = 0xFEFF;

        loop {
            let candidate = self.next_handle;

            // Skip reserved handles (0xFFFE, 0xFFFF, 0)
            if candidate == 0 || candidate >= 0xFEFF {
                self.next_handle = 1;
                attempts += 1;
                if attempts >= MAX_ATTEMPTS {
                    return Err(SmbiosError::OutOfResources);
                }
                continue;
            }

            if !self.is_handle_allocated(candidate) {
                let allocated = candidate;
                self.next_handle = candidate + 1;
                return Ok(allocated);
            }

            self.next_handle += 1;
            attempts += 1;

            // Prevent infinite loop - if we've wrapped around to start
            if attempts >= MAX_ATTEMPTS {
                return Err(SmbiosError::OutOfResources);
            }
        }
    }

    #[allow(dead_code)]
    fn install_configuration_table(&self) -> Result<(), SmbiosError> {
        // This would interact with UEFI Boot Services to install
        // the SMBIOS table in the system configuration table
        // Implementation depends on your UEFI framework

        // For SMBIOS 2.x
        if let Some(_entry_point_32) = &self.entry_point_32 {
            // Install with SMBIOS 2.x GUID
        }

        // For SMBIOS 3.x
        if let Some(_entry_point_64) = &self.entry_point_64 {
            // Install with SMBIOS 3.x GUID
        }

        Ok(())
    }
}

impl SmbiosRecords<'static> for SmbiosManager {
    unsafe fn add(
        &mut self,
        producer_handle: Option<Handle>,
        record: &SmbiosTableHeader,
    ) -> Result<SmbiosHandle, SmbiosError> {
        // Always allocate a new unique handle
        let smbios_handle = self.allocate_handle()?;

        // Create record data (simplified - would need proper string parsing)
        // SAFETY: Caller guarantees that record points to a complete SMBIOS record
        // with at least record.length bytes of valid memory
        let record_size = record.length as usize;
        let mut data = Vec::with_capacity(record_size + 2); // +2 for double null

        unsafe {
            let bytes = core::slice::from_raw_parts(record as *const _ as *const u8, record_size);
            data.extend_from_slice(bytes);
        }

        // Add double null terminator (simplified)
        data.extend_from_slice(&[0, 0]);

        // Ensure the stored header uses the allocated handle so lookups return it
        let mut stored_header = record.clone();
        stored_header.handle = smbios_handle;

        // Calculate string count from the constructed data
        // SAFETY: We just constructed this data above, so it's valid
        let string_count = Self::count_strings_in_record(&data).unwrap_or(0);

        let smbios_record = SmbiosRecord {
            header: stored_header,
            producer_handle,
            data,
            string_count,
            smbios32_table: true,
            smbios64_table: true,
        };

        let _lock = self.lock.lock();
        self.records.push(smbios_record);
        Ok(smbios_handle)
    }

    fn add_from_bytes(
        &mut self,
        producer_handle: Option<Handle>,
        record_data: &[u8],
    ) -> Result<SmbiosHandle, SmbiosError> {
        // Step 1: Validate minimum size for header (at least 4 bytes)
        if record_data.len() < core::mem::size_of::<SmbiosTableHeader>() {
            return Err(SmbiosError::BufferTooSmall);
        }

        // Step 2: Parse and validate header
        let header = unsafe { &*(record_data.as_ptr() as *const SmbiosTableHeader) };

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

        let smbios_record = SmbiosRecord {
            header: record_header,
            producer_handle,
            data,
            string_count,
            smbios32_table: true,
            smbios64_table: true,
        };

        self.records.push(smbios_record);
        Ok(smbios_handle)
    }

    fn update_string(
        &mut self,
        smbios_handle: SmbiosHandle,
        string_number: usize,
        string: &str,
    ) -> Result<(), SmbiosError> {
        Self::validate_string(string)?;
        let _lock = self.lock.lock();

        // Find the record
        let record =
            self.records.iter_mut().find(|r| r.header.handle == smbios_handle).ok_or(SmbiosError::HandleNotFound)?;

        if string_number == 0 || string_number > record.string_count {
            return Err(SmbiosError::InvalidHandle);
        }

        // Parse the existing string pool
        let header_length = record.header.length as usize;
        if record.data.len() < header_length + 2 {
            return Err(SmbiosError::BufferTooSmall);
        }

        // Extract existing strings from the string pool
        let string_pool_start = header_length;
        let string_pool = &record.data[string_pool_start..];

        let mut existing_strings = Vec::new();
        let mut current_string = Vec::new();
        let mut null_count = 0;

        for &byte in string_pool {
            if byte == 0 {
                if !current_string.is_empty() {
                    existing_strings.push(String::from_utf8_lossy(&current_string).into_owned());
                    current_string.clear();
                }
                null_count += 1;
                if null_count >= 2 {
                    break;
                }
            } else {
                null_count = 0;
                current_string.push(byte);
            }
        }

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

    fn remove(&mut self, smbios_handle: SmbiosHandle) -> Result<(), SmbiosError> {
        let _lock = self.lock.lock();

        let pos =
            self.records.iter().position(|r| r.header.handle == smbios_handle).ok_or(SmbiosError::HandleNotFound)?;

        self.records.remove(pos);

        // Optimization: If we removed a handle lower than next_handle,
        // reset next_handle to reuse the freed handle sooner
        if smbios_handle < self.next_handle && (1..0xFEFF).contains(&smbios_handle) {
            self.next_handle = smbios_handle;
        }

        Ok(())
    }

    fn get_next(
        &self,
        smbios_handle: &mut SmbiosHandle,
        record_type: Option<SmbiosType>,
    ) -> Result<(&SmbiosTableHeader, Option<Handle>), SmbiosError> {
        let _lock = self.lock.lock();

        let start_idx = if *smbios_handle == SMBIOS_HANDLE_PI_RESERVED {
            0
        } else {
            self.records
                .iter()
                .position(|r| r.header.handle == *smbios_handle)
                .map(|i| i + 1)
                .unwrap_or(self.records.len())
        };

        for record in &self.records[start_idx..] {
            if let Some(rt) = record_type
                && record.header.record_type != rt
            {
                continue;
            }

            *smbios_handle = record.header.handle;
            return Ok((&record.header, record.producer_handle));
        }

        *smbios_handle = SMBIOS_HANDLE_PI_RESERVED;
        Err(SmbiosError::HandleNotFound)
    }

    fn iter(&self) -> Box<dyn Iterator<Item = &'static SmbiosRecord> + 'static> {
        // We need to use unsafe here because we're extending the lifetime of the iterator.
        // This is safe because the SmbiosManager is 'static and the records vector
        // is only modified through &mut self methods.
        let records_ptr = self.records.as_ptr();
        let len = self.records.len();
        Box::new((0..len).map(move |i| unsafe { &*records_ptr.add(i) }))
    }

    fn version(&self) -> (u8, u8) {
        (self.major_version, self.minor_version)
    }
}

/// SMBIOS table header structure
#[repr(C, packed)]
#[derive(Debug, Clone)]
pub struct SmbiosTableHeader {
    pub record_type: SmbiosType,
    pub length: u8,
    pub handle: SmbiosHandle,
}

impl SmbiosTableHeader {
    pub fn new(record_type: SmbiosType, length: u8, handle: SmbiosHandle) -> Self {
        Self { record_type, length, handle }
    }

    // fn get_smbios_structure_size(&self) -> (usize, usize) {
    //     let size = self.length as usize;
    //     let num_of_string = 0 as usize;
    //     let mut header_bytes = unsafe {
    //         core::slice::from_raw_parts(
    //             header as *const _ as *const u8,
    //             header.length as usize,
    //         )
    //     };
    //     let mut ptr = *self + self.length;
    //     let mut byte = *ptr;
    //     let mut next_byte = unsafe {ptr.offset(1)};
    //     while byte != 0 || next_byte != 0 {
    //         size += 1;
    //         ptr += 1;
    //     }

    // }
}

/// Internal SMBIOS record representation
pub struct SmbiosRecord {
    pub header: SmbiosTableHeader,
    pub producer_handle: Option<Handle>,
    pub data: Vec<u8>, // Complete record including strings
    // record_size: usize,
    string_count: usize,
    pub smbios32_table: bool,
    pub smbios64_table: bool,
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

    pub fn add_field<T: Copy>(mut self, value: T) -> Self {
        let bytes = unsafe { core::slice::from_raw_parts(&value as *const T as *const u8, core::mem::size_of::<T>()) };
        self.data.extend_from_slice(bytes);
        self
    }

    pub fn add_string(mut self, s: String) -> Result<Self, SmbiosError> {
        SmbiosManager::validate_string(&s)?;
        self.strings.push(s);
        Ok(self)
    }

    pub fn build(self) -> Result<Vec<u8>, SmbiosError> {
        let mut record = Vec::new();

        // Add header
        let header = SmbiosTableHeader {
            record_type: self.record_type,
            length: (core::mem::size_of::<SmbiosTableHeader>() + self.data.len()) as u8,
            handle: SMBIOS_HANDLE_PI_RESERVED,
        };

        let header_bytes = unsafe {
            core::slice::from_raw_parts(&header as *const _ as *const u8, core::mem::size_of::<SmbiosTableHeader>())
        };
        record.extend_from_slice(header_bytes);

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
    const PROTOCOL_GUID: efi::Guid = efi::Guid::from_fields(
        0x03583ff6,
        0xcb36,
        0x4940,
        0x94,
        0x7e,
        &[0xb9, 0xb3, 0x9f, 0x4a, 0xfa, 0xf7], // ✅ Corrected GUID
    );
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

    #[allow(dead_code)]
    extern "efiapi" fn add_ext(
        _protocol: *const SmbiosProtocol,
        _producer_handle: efi::Handle,
        smbios_handle: *mut SmbiosHandle,
        record: *const SmbiosTableHeader,
    ) -> efi::Status {
        // Safety checks
        if smbios_handle.is_null() || record.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }

        // Get global manager and call add_record
        // Implementation would depend on your global state management
        efi::Status::SUCCESS
    }

    #[allow(dead_code)]
    extern "efiapi" fn update_string_ext(
        _protocol: *const SmbiosProtocol,
        _smbios_handle: *mut SmbiosHandle,
        _string_number: *mut usize,
        _string: *const c_char,
    ) -> efi::Status {
        // Implementation similar to add_ext
        efi::Status::SUCCESS
    }

    #[allow(dead_code)]
    extern "efiapi" fn remove_ext(_protocol: *const SmbiosProtocol, _smbios_handle: SmbiosHandle) -> efi::Status {
        // Implementation similar to add_ext
        efi::Status::SUCCESS
    }

    #[allow(dead_code)]
    extern "efiapi" fn get_next_ext(
        _protocol: *const SmbiosProtocol,
        _smbios_handle: *mut SmbiosHandle,
        _record_type: *mut SmbiosType,
        _record: *mut *mut SmbiosTableHeader,
        _producer_handle: *mut efi::Handle,
    ) -> efi::Status {
        // Implementation similar to add_ext
        efi::Status::SUCCESS
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use crate::smbios_record::SmbiosRecordStructure;
    use crate::smbios_record::Type0PlatformFirmwareInformation;
    use std::{format, println, vec};
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
        println!("record - {:?}", record);
        // assert_eq!(record, b"\x01\x06\xfe\xff\x01\x02ACME Corp\x00SuperServer 3000\x00\x00");
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

        // Build a SmbiosTableHeader from the serialized bytes header area
        // We can read the header from the bytes since serialization places header first
        let header_size = core::mem::size_of::<SmbiosTableHeader>();
        assert!(record_bytes.len() >= header_size + 2);

        let header_slice = &record_bytes[..header_size];
        let record_header: SmbiosTableHeader = unsafe {
            // Transmute the first bytes into a header (safe in test because sizes match)
            core::ptr::read_unaligned(header_slice.as_ptr() as *const SmbiosTableHeader)
        };

        // Add to manager and get the assigned handle
        let handle = unsafe { manager.add(None, &record_header).expect("add failed") };

        // Retrieve using get_next
        let mut search_handle = SMBIOS_HANDLE_PI_RESERVED;
        let (found_header, _producer) = manager
            .get_next(&mut search_handle, Some(Type0PlatformFirmwareInformation::RECORD_TYPE))
            .expect("get_next failed");

        println!("Type0PlatformFirmwareInformation - {:02x?}", record_bytes);
        println!("{}", record_bytes.iter().map(|b| format!(" 0x{:02x}", b)).collect::<String>());
        assert_eq!(found_header.record_type, Type0PlatformFirmwareInformation::RECORD_TYPE);
        assert_eq!(search_handle, handle);
    }
}
