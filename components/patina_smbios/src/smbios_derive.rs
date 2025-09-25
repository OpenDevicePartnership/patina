use hashbrown::HashSet;
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
    type Iter: Iterator<Item = &'a SmbiosRecord>;

    /// Adds an SMBIOS record to the SMBIOS table.
    /// If handle is SMBIOS_HANDLE_PI_RESERVED (0xFFFE), a unique handle will be assigned.
    fn add(
        &mut self,
        producer_handle: Option<Handle>,
        smbios_handle: &mut SmbiosHandle,
        record: &SmbiosTableHeader,
    ) -> Result<(), SmbiosError>;

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
    fn iter(&self) -> Self::Iter;

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
    allocated_handles: HashSet<SmbiosHandle>,
    major_version: u8,
    minor_version: u8,
    entry_point_32: Option<Box<SmbiosEntryPoint>>,
    entry_point_64: Option<Box<Smbios30EntryPoint>>,
    table_32_address: Option<PhysicalAddress>,
    table_64_address: Option<PhysicalAddress>,
    lock: Mutex<()>,
}

impl SmbiosManager {
    pub fn new(major_version: u8, minor_version: u8) -> Self {
        Self {
            records: Vec::new(),
            allocated_handles: HashSet::new(),
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

    fn allocate_handle(&mut self) -> Result<SmbiosHandle, SmbiosError> {
        for handle in 1..0xFF00 {
            if !self.allocated_handles.contains(&handle) {
                self.allocated_handles.insert(handle);
                return Ok(handle);
            }
        }
        Err(SmbiosError::OutOfResources)
    }

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
    type Iter = core::slice::Iter<'static, SmbiosRecord>;

    fn add(
        &mut self,
        producer_handle: Option<Handle>,
        smbios_handle: &mut SmbiosHandle,
        record: &SmbiosTableHeader,
    ) -> Result<(), SmbiosError> {
        // Assign handle if needed
        if *smbios_handle == SMBIOS_HANDLE_PI_RESERVED {
            *smbios_handle = self.allocate_handle()?;
        } else if self.allocated_handles.contains(smbios_handle) {
            return Err(SmbiosError::HandleAlreadyInUse);
        } else {
            self.allocated_handles.insert(*smbios_handle);
        }

        // Create record data (simplified - would need proper string parsing)
        // let record_size = record.len() as usize;
        let record_size = core::mem::size_of::<SmbiosTableHeader>();
        let mut data = Vec::with_capacity(record_size + 2); // +2 for double null

        unsafe {
            let bytes = core::slice::from_raw_parts(&record as *const _ as *const u8, record_size);
            data.extend_from_slice(bytes);
        }

        // Add double null terminator (simplified)
        data.extend_from_slice(&[0, 0]);

        // Ensure the stored header uses the (possibly allocated) handle so lookups return it
        let mut stored_header = record.clone();
        stored_header.handle = *smbios_handle;

        let smbios_record = SmbiosRecord {
            header: stored_header,
            producer_handle,
            data,
            string_count: 0, // Would be calculated from actual strings
            smbios32_table: true,
            smbios64_table: true,
        };

        let _lock = self.lock.lock();
        self.records.push(smbios_record);
        Ok(())
    }

    fn update_string(
        &mut self,
        smbios_handle: SmbiosHandle,
        string_number: usize,
        string: &str,
    ) -> Result<(), SmbiosError> {
        Self::validate_string(string)?;
        //TODO fix build error let _lock = self.lock.lock().unwrap();

        // Find the record
        let record =
            self.records.iter_mut().find(|r| r.header.handle == smbios_handle).ok_or(SmbiosError::HandleNotFound)?;

        if string_number == 0 || string_number > record.string_count {
            return Err(SmbiosError::InvalidHandle);
        }

        // Update string (simplified implementation)
        // Real implementation would parse and rebuild the string section
        Ok(())
    }

    fn remove(&mut self, smbios_handle: SmbiosHandle) -> Result<(), SmbiosError> {
        //TODO fix build error let _lock = self.lock.lock().unwrap();

        let pos =
            self.records.iter().position(|r| r.header.handle == smbios_handle).ok_or(SmbiosError::HandleNotFound)?;

        self.records.remove(pos);
        self.allocated_handles.remove(&smbios_handle);
        Ok(())
    }

    fn get_next(
        &self,
        smbios_handle: &mut SmbiosHandle,
        record_type: Option<SmbiosType>,
    ) -> Result<(&SmbiosTableHeader, Option<Handle>), SmbiosError> {
        //TODO fix build error let _lock = self.lock.lock().unwrap();

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
            if let Some(rt) = record_type {
                if record.header.record_type != rt {
                    continue;
                }
            }

            *smbios_handle = record.header.handle;
            return Ok((&record.header, record.producer_handle));
        }

        *smbios_handle = SMBIOS_HANDLE_PI_RESERVED;
        Err(SmbiosError::HandleNotFound)
    }

    fn iter(&self) -> Self::Iter {
        // This is a simplified implementation
        // Real implementation would need proper lifetime management
        unsafe { core::mem::transmute(self.records.iter()) }
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

type SmbiosAdd =
    extern "efiapi" fn(*const SmbiosProtocol, efi::Handle, *mut SmbiosHandle, *const SmbiosTableHeader) -> efi::Status;

type SmbiosUpdateString =
    extern "efiapi" fn(*const SmbiosProtocol, *mut SmbiosHandle, *mut usize, *const c_char) -> efi::Status;

type SmbiosRemove = extern "efiapi" fn(*const SmbiosProtocol, SmbiosHandle) -> efi::Status;

type SmbiosGetNext = extern "efiapi" fn(
    *const SmbiosProtocol,
    *mut SmbiosHandle,
    *mut SmbiosType,
    *mut *mut SmbiosTableHeader,
    *mut efi::Handle,
) -> efi::Status;

impl SmbiosProtocol {
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

        // Get global manager and call add_record
        // Implementation would depend on your global state management
        efi::Status::SUCCESS
    }

    extern "efiapi" fn update_string_ext(
        _protocol: *const SmbiosProtocol,
        smbios_handle: *mut SmbiosHandle,
        string_number: *mut usize,
        string: *const c_char,
    ) -> efi::Status {
        // Implementation similar to add_ext
        efi::Status::SUCCESS
    }

    extern "efiapi" fn remove_ext(_protocol: *const SmbiosProtocol, smbios_handle: SmbiosHandle) -> efi::Status {
        // Implementation similar to add_ext
        efi::Status::SUCCESS
    }

    extern "efiapi" fn get_next_ext(
        _protocol: *const SmbiosProtocol,
        smbios_handle: *mut SmbiosHandle,
        record_type: *mut SmbiosType,
        record: *mut *mut SmbiosTableHeader,
        producer_handle: *mut efi::Handle,
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
    use std::{format, print, println, vec};
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

        // Add to manager
        let mut handle = SMBIOS_HANDLE_PI_RESERVED;
        manager.add(None, &mut handle, &record_header).expect("add failed");

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
