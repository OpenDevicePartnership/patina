#![no_std]
use hashbrown::HashSet;
extern crate alloc;
use alloc::vec::Vec;
use alloc::boxed::Box;
use alloc::string::String;
use r_efi::efi::PhysicalAddress;
use r_efi::efi::Handle;
use spin::Mutex;
//use core::slice;

//typedef struct {
//  SMBIOS_TYPE      Type;
//  UINT8            Length;
//  SMBIOS_HANDLE    Handle;
//} SMBIOS_STRUCTURE;
//typedef UINT8 SMBIOS_TABLE_STRING;
//typedef UINT8 SMBIOS_TYPE
//typedef UINT16 SMBIOS_HANDLE
//typedef SMBIOS_TABLE_STRING  EFI_SMBIOS_STRING;
//typedef SMBIOS_TYPE          EFI_SMBIOS_TYPE;
//typedef SMBIOS_HANDLE        EFI_SMBIOS_HANDLE;
//typedef SMBIOS_STRUCTURE     EFI_SMBIOS_TABLE_HEADER;
/// SMBIOS handle type
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
    
    fn build_record_with_strings(
        header: &SmbiosTableHeader,
        strings: &[&str],
    ) -> Result<Vec<u8>, SmbiosError> {
        // Validate all strings first
        for s in strings {
            Self::validate_string(s)?;
        }
        
        let mut record = Vec::new();
        
        // Add the structured data
        let header_bytes = unsafe {
            core::slice::from_raw_parts(
                header as *const _ as *const u8,
                header.length as usize,
            )
        };
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
        header: &SmbiosTableHeader,
    ) -> Result<(), SmbiosError> {
        //TODO fix build error let _lock = self.lock.lock().unwrap();

        // Build record
        let record = Self::build_record_with_strings(header, &[])?;

        // Validate record
        if record.len() == 0 {
            return Err(SmbiosError::InvalidParameter);
        }

        // Assign handle if needed
        if *smbios_handle == SMBIOS_HANDLE_PI_RESERVED {
            *smbios_handle = self.allocate_handle()?;
        } else if self.allocated_handles.contains(smbios_handle) {
            return Err(SmbiosError::HandleAlreadyInUse);
        } else {
            self.allocated_handles.insert(*smbios_handle);
        }

        // Create record data (simplified - would need proper string parsing)
        let record_size = record.len() as usize;
        let mut data = Vec::with_capacity(record_size + 2); // +2 for double null
        
        unsafe {
            let bytes = core::slice::from_raw_parts(
                &record as *const _ as *const u8,
                record_size,
            );
            data.extend_from_slice(bytes);
        }
        
        // Add double null terminator (simplified)
        data.extend_from_slice(&[0, 0]);

        let smbios_record = SmbiosRecord {
            header: header.clone(),
            producer_handle,
            data,
            string_count: 0, // Would be calculated from actual strings
            smbios32_table: true,
            smbios64_table: true,
        };

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
        let record = self.records
            .iter_mut()
            .find(|r| r.header.handle == smbios_handle)
            .ok_or(SmbiosError::HandleNotFound)?;

        if string_number == 0 || string_number > record.string_count {
            return Err(SmbiosError::InvalidHandle);
        }

        // Update string (simplified implementation)
        // Real implementation would parse and rebuild the string section
        Ok(())
    }

    fn remove(&mut self, smbios_handle: SmbiosHandle) -> Result<(), SmbiosError> {
        //TODO fix build error let _lock = self.lock.lock().unwrap();
        
        let pos = self.records
            .iter()
            .position(|r| r.header.handle == smbios_handle)
            .ok_or(SmbiosError::HandleNotFound)?;

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

/// Internal SMBIOS record representation
pub struct SmbiosRecord {
    pub header: SmbiosTableHeader,
    pub producer_handle: Option<Handle>,
    pub data: Vec<u8>, // Complete record including strings
    pub string_count: usize,
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
        Self {
            record_type,
            data: Vec::new(),
            strings: Vec::new(),
        }
    }
    
    pub fn add_field<T: Copy>(mut self, value: T) -> Self {
        let bytes = unsafe {
            core::slice::from_raw_parts(
                &value as *const T as *const u8,
                core::mem::size_of::<T>(),
            )
        };
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
            core::slice::from_raw_parts(
                &header as *const _ as *const u8,
                core::mem::size_of::<SmbiosTableHeader>(),
            )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_SmbiosRecordBuilder() {
        let record = SmbiosRecordBuilder::new(1) // System Information
            .add_field(1u8)  // manufacturer string index
            .add_field(2u8)  // product name string index
            .add_string("ACME Corp".to_string())?
            .add_string("SuperServer 3000".to_string())?
            .build()?;
    }
}
