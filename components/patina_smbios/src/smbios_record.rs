extern crate alloc;
use alloc::vec::Vec;
use alloc::string::String;
use crate::smbios_derive::{SmbiosTableHeader, SmbiosError, SMBIOS_HANDLE_PI_RESERVED, SMBIOS_STRING_MAX_LENGTH};


macro_rules! vec {
    () => {
        Vec::new()
    };
    ( $( $x:expr ),* ) => {{
        let mut temp_vec = Vec::new();
        $(
            temp_vec.push($x);
        )*
        temp_vec
    }};
    ( $( $x:expr ),+ , ) => {
        vec![ $( $x ),* ]
    };
    ( $element:expr ; $n:expr ) => {{
        let mut temp_vec = Vec::with_capacity($n);
        for _ in 0..$n {
            temp_vec.push($element);
        }
        temp_vec
    }};
}

/// Base trait for SMBIOS record structures with generic serialization
pub trait SmbiosRecordStructure {
    /// The SMBIOS record type number
    const RECORD_TYPE: u8;
    
    /// Convert the structure to a complete SMBIOS record byte array
    // fn to_bytes(&self) -> Vec<u8>;
    fn to_bytes(&self) -> Vec<u8> 
        // where Self: SmbiosFieldLayout
        where Self: SmbiosFieldLayout, Self: Sized
    {
        SmbiosSerializer::serialize(self)
    }
    
    /// Validate the structure before serialization
    fn validate(&self) -> Result<(), SmbiosError>;
    
    /// Get the string pool for this record
    fn string_pool(&self) -> &[String];
    
    /// Get mutable access to the string pool
    fn string_pool_mut(&mut self) -> &mut Vec<String>;
}

/// Generic SMBIOS record serializer using reflection-like techniques
pub struct SmbiosSerializer;

impl SmbiosSerializer {
    /// Serialize any SMBIOS record structure to bytes
    pub fn serialize<T: SmbiosRecordStructure + SmbiosFieldLayout>(record: &T) -> Vec<u8> {
        let mut bytes = Vec::new();
        
        // Step 1: Calculate structured data size using field layout
        let field_layout = T::field_layout();
        let structured_size = core::mem::size_of::<SmbiosTableHeader>() + field_layout.total_size();
        
        // Step 2: Create header
        let header = SmbiosTableHeader {
            record_type: T::RECORD_TYPE,
            length: structured_size as u8,
            handle: SMBIOS_HANDLE_PI_RESERVED,
        };
        
        // Step 3: Serialize header
        bytes.extend_from_slice(&Self::serialize_header(&header));
        
        // Step 4: Serialize structured fields using generic field serialization
        bytes.extend_from_slice(&Self::serialize_fields(record, &field_layout));
        
        // Step 5: Serialize string pool
        bytes.extend_from_slice(&Self::serialize_string_pool(record.string_pool()));
        
        bytes
    }
    
    fn serialize_header(header: &SmbiosTableHeader) -> [u8; 4] {
        [
            header.record_type,
            header.length,
            (header.handle & 0xFF) as u8,
            ((header.handle >> 8) & 0xFF) as u8,
        ]
    }
    
    fn serialize_fields<T: SmbiosRecordStructure + SmbiosFieldLayout>(
        record: &T, 
        layout: &FieldLayout
    ) -> Vec<u8> {
        let mut bytes = Vec::new();
        
        // Use the field layout to serialize each field generically
        for field_info in &layout.fields {
            match field_info.field_type {
                FieldType::U8(offset) => {
                    let value = unsafe { 
                        *((record as *const T as *const u8).add(offset) as *const u8)
                    };
                    bytes.push(value);
                }
                FieldType::U16(offset) => {
                    let value = unsafe { 
                        *((record as *const T as *const u8).add(offset) as *const u16)
                    };
                    bytes.extend_from_slice(&value.to_le_bytes());
                }
                FieldType::U32(offset) => {
                    let value = unsafe { 
                        *((record as *const T as *const u8).add(offset) as *const u32)
                    };
                    bytes.extend_from_slice(&value.to_le_bytes());
                }
                FieldType::U64(offset) => {
                    let value = unsafe { 
                        *((record as *const T as *const u8).add(offset) as *const u64)
                    };
                    bytes.extend_from_slice(&value.to_le_bytes());
                }
                FieldType::ByteArray { offset, len } => {
                    let slice = unsafe {
                        core::slice::from_raw_parts(
                            (record as *const T as *const u8).add(offset),
                            len
                        )
                    };
                    bytes.extend_from_slice(slice);
                }
            }
        }
        
        bytes
    }
    
    fn serialize_string_pool(strings: &[String]) -> Vec<u8> {
        let mut bytes = Vec::new();
        
        if strings.is_empty() {
            bytes.extend_from_slice(&[0, 0]);
        } else {
            for string in strings {
                if !string.is_empty() {
                    bytes.extend_from_slice(string.as_bytes());
                }
                bytes.push(0);
            }
            bytes.push(0); // Double null terminator
        }
        
        bytes
    }
}

/// Field layout description for generic serialization
pub trait SmbiosFieldLayout {
    fn field_layout() -> FieldLayout;
}

#[derive(Debug, Clone)]
pub struct FieldLayout {
    pub fields: Vec<FieldInfo>,
}

impl FieldLayout {
    pub fn total_size(&self) -> usize {
        self.fields.iter().map(|f| f.size()).sum()
    }
}

#[derive(Debug, Clone)]
pub struct FieldInfo {
    pub name: &'static str,
    pub field_type: FieldType,
}

#[derive(Debug, Clone)]
pub enum FieldType {
    U8(usize),                           // offset
    U16(usize),                          // offset
    U32(usize),                          // offset  
    U64(usize),                          // offset
    ByteArray { offset: usize, len: usize }, // offset, length
}

impl FieldInfo {
    pub fn size(&self) -> usize {
        match self.field_type {
            FieldType::U8(_) => 1,
            FieldType::U16(_) => 2,
            FieldType::U32(_) => 4,
            FieldType::U64(_) => 8,
            FieldType::ByteArray { len, .. } => len,
        }
    }
}

/// Macro to automatically generate field layout for SMBIOS records
macro_rules! impl_smbios_field_layout {
    ($struct_name:ident, $($field_name:ident: $field_type:ident),* $(,)?) => {
        impl SmbiosFieldLayout for $struct_name {
            fn field_layout() -> FieldLayout {
                use core::mem::{offset_of};
                // use core::mem::{offset_of, size_of};
                
                FieldLayout {
                    fields: vec![
                        $(impl_smbios_field_layout!(@field_info $struct_name, $field_name, $field_type),)*
                    ],
                }
            }
        }
    };
    
    (@field_info $struct_name:ident, $field_name:ident, u8) => {
        FieldInfo {
            name: stringify!($field_name),
            field_type: FieldType::U8(offset_of!($struct_name, $field_name)),
        }
    };
    
    (@field_info $struct_name:ident, $field_name:ident, u16) => {
        FieldInfo {
            name: stringify!($field_name),
            field_type: FieldType::U16(offset_of!($struct_name, $field_name)),
        }
    };
    
    (@field_info $struct_name:ident, $field_name:ident, u32) => {
        FieldInfo {
            name: stringify!($field_name),
            field_type: FieldType::U32(offset_of!($struct_name, $field_name)),
        }
    };
    
    (@field_info $struct_name:ident, $field_name:ident, u64) => {
        FieldInfo {
            name: stringify!($field_name),
            field_type: FieldType::U64(offset_of!($struct_name, $field_name)),
        }
    };
    
    (@field_info $struct_name:ident, $field_name:ident, uuid) => {
        FieldInfo {
            name: stringify!($field_name),
            field_type: FieldType::ByteArray { 
                offset: offset_of!($struct_name, $field_name), 
                len: 16 
            },
        }
    };
}

/// Type 0: Platform Firmware Information (BIOS Information)
// #[repr(C, packed)]
pub struct Type0PlatformFirmwareInformation {
    pub header: SmbiosTableHeader,
    pub vendor: u8,                           // String index
    pub firmware_version: u8,                 // String index
    pub bios_starting_address_segment: u16,
    pub firmware_release_date: u8,            // String index
    pub firmware_rom_size: u8,
    pub characteristics: u64,
    pub characteristics_ext1: u8,
    pub characteristics_ext2: u8,
    pub system_bios_major_release: u8,
    pub system_bios_minor_release: u8,
    pub embedded_controller_major_release: u8,
    pub embedded_controller_minor_release: u8,
    pub extended_bios_rom_size: u16,
    
    // Integrated string pool
    pub string_pool: Vec<String>,
}

// Generic field layout - much simpler than custom to_bytes()!
impl_smbios_field_layout!(Type0PlatformFirmwareInformation,
    vendor: u8,
    firmware_version: u8,
    bios_starting_address_segment: u16,
    firmware_release_date: u8,
    firmware_rom_size: u8,
    characteristics: u64,
    characteristics_ext1: u8,
    characteristics_ext2: u8,
    system_bios_major_release: u8,
    system_bios_minor_release: u8,
    embedded_controller_major_release: u8,
    embedded_controller_minor_release: u8,
    extended_bios_rom_size: u16,
);

impl Type0PlatformFirmwareInformation {
    /// Create a new Type 0 record with default values
    pub fn new() -> Self {
        Self {
            header: SmbiosTableHeader {
                record_type: 0,
                length: 0, // Will be calculated in to_bytes()
                handle: SMBIOS_HANDLE_PI_RESERVED,
            },
            vendor: 1,                           // First string in pool
            firmware_version: 2,                 // Second string in pool
            bios_starting_address_segment: 0xE000,
            firmware_release_date: 3,            // Third string in pool
            firmware_rom_size: 0x0F,            // Default: 1MB
            characteristics: 0x08,              // PCI supported
            characteristics_ext1: 0x01,         // ACPI supported
            characteristics_ext2: 0x00,
            system_bios_major_release: 1,
            system_bios_minor_release: 0,
            embedded_controller_major_release: 0xFF, // Not supported
            embedded_controller_minor_release: 0xFF, // Not supported
            extended_bios_rom_size: 0x0000,
            string_pool: vec![
                String::from("Default Vendor"),
                String::from("1.0.0"),
                String::from("01/01/2025"),
            ],
        }
    }
    
    /// Set vendor information (updates both string pool and index)
    pub fn with_vendor(mut self, vendor: String) -> Result<Self, SmbiosError> {
        Self::validate_string(&vendor)?;
        if self.string_pool.is_empty() {
            self.string_pool.push(vendor);
            self.vendor = 1;
        } else {
            self.string_pool[0] = vendor;
        }
        Ok(self)
    }

    /// Set firmware version (updates both string pool and index)
    pub fn with_firmware_version(mut self, version: String) -> Result<Self, SmbiosError> {
        Self::validate_string(&version)?;
        while self.string_pool.len() < 2 {
            self.string_pool.push(String::new());
        }
        self.string_pool[1] = version;
        self.firmware_version = 2;
        Ok(self)
    }
    
    /// Set release date (updates both string pool and index)
    pub fn with_release_date(mut self, date: String) -> Result<Self, SmbiosError> {
        Self::validate_string(&date)?;
        while self.string_pool.len() < 3 {
            self.string_pool.push(String::new());
        }
        self.string_pool[2] = date;
        self.firmware_release_date = 3;
        Ok(self)
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
}

impl SmbiosRecordStructure for Type0PlatformFirmwareInformation {
    const RECORD_TYPE: u8 = 0;
    
    // to_bytes() is provided by default implementation using generic serializer!
    
    fn validate(&self) -> Result<(), SmbiosError> {
        // Validate all strings
        for string in &self.string_pool {
            Self::validate_string(string)?;
        }
        
        // Validate string indices point to valid strings
        if self.vendor > 0 && (self.vendor as usize) > self.string_pool.len() {
            return Err(SmbiosError::InvalidParameter);
        }
        if self.firmware_version > 0 && (self.firmware_version as usize) > self.string_pool.len() {
            return Err(SmbiosError::InvalidParameter);
        }
        if self.firmware_release_date > 0 && (self.firmware_release_date as usize) > self.string_pool.len() {
            return Err(SmbiosError::InvalidParameter);
        }
        
        Ok(())
    }
    
    fn string_pool(&self) -> &[String] {
        &self.string_pool
    }
    
    fn string_pool_mut(&mut self) -> &mut Vec<String> {
        &mut self.string_pool
    }
}

/// Type 1: System Information
// #[repr(C, packed)]
pub struct Type1SystemInformation {
    pub header: SmbiosTableHeader,
    pub manufacturer: u8,         // String index
    pub product_name: u8,         // String index
    pub version: u8,              // String index
    pub serial_number: u8,        // String index
    pub uuid: [u8; 16],
    pub wake_up_type: u8,
    pub sku_number: u8,           // String index
    pub family: u8,               // String index
    
    // Integrated string pool
    pub string_pool: Vec<String>,
}

impl Type1SystemInformation {
    pub fn new() -> Self {
        Self {
            header: SmbiosTableHeader {
                record_type: 1,
                length: 0, // Will be calculated
                handle: SMBIOS_HANDLE_PI_RESERVED,
            },
            manufacturer: 1,
            product_name: 2,
            version: 3,
            serial_number: 4,
            uuid: [0; 16], // Should be set by implementation
            wake_up_type: 0x06, // Power switch
            sku_number: 5,
            family: 6,
            string_pool: vec![
                String::from("System Manufacturer"),
                String::from("System Product"),
                String::from("1.0"),
                String::from("SystemSerial123"),
                String::from("SystemSKU"),
                String::from("System Family"),
            ],
        }
    }
}

// With the generic serializer, Type 1 becomes trivial to implement:
impl_smbios_field_layout!(Type1SystemInformation,
    manufacturer: u8,
    product_name: u8,
    version: u8,
    serial_number: u8,
    uuid: uuid,           // Special uuid type handles 16-byte arrays
    wake_up_type: u8,
    sku_number: u8,
    family: u8,
);

impl SmbiosRecordStructure for Type1SystemInformation {
    const RECORD_TYPE: u8 = 1;
    
    fn validate(&self) -> Result<(), SmbiosError> {
        // Standard string validation (could even be provided by a derive macro)
        for string in &self.string_pool {
            if string.len() > SMBIOS_STRING_MAX_LENGTH {
                return Err(SmbiosError::StringTooLong);
            }
            if string.contains('\0') {
                return Err(SmbiosError::InvalidParameter);
            }
        }
        Ok(())
    }
    
    fn string_pool(&self) -> &[String] {
        &self.string_pool
    }
    
    fn string_pool_mut(&mut self) -> &mut Vec<String> {
        &mut self.string_pool
    }
}

// /// Even Better: Derive Macro Approach
// /// 
// /// For maximum scalability, we could provide derive macros that generate 
// /// everything automatically:

// use smbios_derive::SmbiosRecord;

// #[derive(SmbiosRecord)]
// #[smbios(record_type = 2)]
// pub struct Type2BaseboardInformation {
//     pub manufacturer: u8,        // String index
//     pub product: u8,             // String index  
//     pub version: u8,             // String index
//     pub serial_number: u8,       // String index
//     pub asset_tag: u8,           // String index
//     pub feature_flags: u8,
//     pub location_in_chassis: u8, // String index
//     pub chassis_handle: u16,
//     pub board_type: u8,
//     pub contained_object_handles: u8,
    
//     #[smbios(string_pool)]
//     pub strings: Vec<String>,
// }

// // The derive macro would automatically generate:
// // - impl SmbiosFieldLayout 
// // - impl SmbiosRecordStructure with proper RECORD_TYPE
// // - Standard validation for strings
// // - String pool accessors

// /// Type 3: System Enclosure - another example showing how simple it becomes
// #[derive(SmbiosRecord)] 
// #[smbios(record_type = 3)]
// pub struct Type3SystemEnclosure {
//     pub manufacturer: u8,         // String index
//     pub enclosure_type: u8,
//     pub version: u8,              // String index
//     pub serial_number: u8,        // String index
//     pub asset_tag_number: u8,     // String index
//     pub bootup_state: u8,
//     pub power_supply_state: u8,
//     pub thermal_state: u8,
//     pub security_status: u8,
//     pub oem_defined: u32,
//     pub height: u8,
//     pub number_of_power_cords: u8,
//     pub contained_element_count: u8,
//     pub contained_element_record_length: u8,
    
//     #[smbios(string_pool)]
//     pub strings: Vec<String>,
// }

// // That's it! No manual serialization code needed.

// /// OEM Record becomes equally simple:
// #[derive(SmbiosRecord)]
// #[smbios(record_type = 0x80)]
// pub struct OemCustomRecord {
//     pub oem_field1: u32,
//     pub oem_field2: u16,
//     pub oem_string_ref: u8,       // String index
//     #[smbios(array_len = 8)]
//     pub reserved: [u8; 8],
    
//     #[smbios(string_pool)]
//     pub strings: Vec<String>,
// }