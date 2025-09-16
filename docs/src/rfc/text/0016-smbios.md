# RFC: `SMBIOS`

This RFC proposes a Rust-based interface for SMBIOS table management, providing the functionality described in the
`EFI_SMBIOS_PROTOCOL` service defined in the UEFI Specification.

It introduces an `SmbiosRecords` trait to manage addition, removal, updates, and retrieval of SMBIOS records,
encapsulating the SMBIOS protocol behind a safe, idiomatic Rust API.

The goal is to maintain compatibility with the UEFI specification while improving memory safety and simplifying
integration for components.

## Change Log

- 2025-07-28: Initial RFC created.

## Motivation

The System Management BIOS (SMBIOS) specification defines data structures and access methods that allow hardware and system
vendors to provide management applications with system hardware information. This RFC proposes a pure Rust interface for
existing SMBIOS capabilities, replacing the C-based implementations while producing protocols that align with the UEFI
specification. This provides a simpler, safer Rust-based interface while maintaining required SMBIOS functionality.

### Scope

The `SmbiosRecords` service implements equivalent functionality for the following protocol:

- `EFI_SMBIOS_PROTOCOL`
  - `Add`
  - `UpdateString`
  - `Remove`
  - `GetNext`
  - `MajorVersion`
  - `MinorVersion`

## Technology Background

### SMBIOS

SMBIOS within UEFI provides a standardized interface for firmware
to convey system hardware configuration information to the operating system.
This information is organized into a set of structured tables containing details about
the system's hardware components, configuration, and capabilities.

SMBIOS supports both 32-bit (SMBIOS 2.x) and 64-bit (SMBIOS 3.x) table formats,
with different size limitations and entry point structures.

For more information on the format and arrangement of these tables,
see the SMBIOS specification and the UEFI specification on SMBIOS protocols.

### Protocols

The UEFI Forum Specifications expose the primary protocol for interacting with SMBIOS data:

- The SMBIOS Protocol manages individual SMBIOS records and strings.
  - [EFI_SMBIOS_PROTOCOL](https://uefi.org/specs/PI/1.9/V5_SMBIOS_Protocol.html)

## Goals

Create an idiomatic Rust API for SMBIOS-related protocols (*see [Motivation - Scope](#scope)*).

## Requirements

1. The API should provide all necessary SMBIOS functionality as a service to components
2. The API should utilize Rust best practices, particularly memory safety and error handling
3. The SMBIOS service should produce protocols equivalent to the current C implementations, preserving existing C functionality
4. Support both SMBIOS 2.x (32-bit) and SMBIOS 3.x (64-bit) table formats
5. Provide safe string manipulation for SMBIOS records

## Unresolved Questions

Should the API expose separate interfaces for 32-bit and 64-bit SMBIOS tables, or handle this internally?
Summary: Prefer a unified public API that handles 32/64 internally, plus an opt-in low-level escape hatch for experts.

Separate interfaces (explicit 32-bit and 64-bit APIs)

Pros:

- Strong correctness and clarity: callers pick the exact format.
- Compile-time signal for address sizes and layout differences.
- Easier format-specific validation and optimizations.
- Good for low-level tooling that needs precise control.

Cons:

- Larger, duplicated API surface; more to maintain and test.
- Forces agnostic consumers to write wrappers or choose formats.
- Encourages fragmented code paths and potential divergence.
- Harder to support mixed (publish both) scenarios.

Unified API that handles 32/64 internally

Pros:

- Ergonomic: most users don’t need to care about table width.
- Simpler docs and adoption; fewer call-site code paths.
- Stable logical model with internal format translation.
- Easier to evolve for future SMBIOS updates.

Cons:

- More internal complexity and runtime branching.
- Advanced users lose direct control without escape hatches.
- Version-specific constraints can be hidden/surprising.
- Requires thorough tests to catch format-specific bugs.

Recommendation

Patina exposes a single, ergonomic SMBIOS API that's format-agnostic by default. The SmbiosRecords service lets
components add, update, remove, and enumerate records without choosing between SMBIOS 2.x (32-bit) and 3.x (64-bit).

The service selects and publishes the appropriate tables per platform and policy. For advanced firmware tooling, an
opt-in "raw" module provides explicit 32/64 entry-point construction and publishing primitives, so experts retain
full control when needed.

Tiny API sketch

```rust
  /// Optional hint when you need to force a target format.
  #[derive(Copy, Clone, Debug)]
  pub enum SmbiosTarget {
      Auto,     // default: publish per platform policy (may publish both)
      Force32,  // SMBIOS 2.x entry point and table
      Force64,  // SMBIOS 3.x entry point and table
  }

  pub trait SmbiosRecordsExt: SmbiosRecords {
      /// Same as `add`, but allows callers to influence the target format.
      fn add_with_target(
          &mut self,
          producer_handle: Option<Handle>,
          smbios_handle: &mut SmbiosHandle,
          record: &SmbiosTableHeader,
          target: SmbiosTarget,
      ) -> Result<(), SmbiosError>;
  }

// Public, ergonomic usage (format-agnostic by default)
fn produce_record(smbios: &mut dyn SmbiosRecords) -> Result<(), SmbiosError> {
    let record = SmbiosTableHeader {
        record_type: 0,
        length: core::mem::size_of::<SmbiosTableHeader>() as u8,
        handle: SMBIOS_HANDLE_PI_RESERVED,
    };

    // Ergonomic path (Auto)
    let handle = smbios.add(None, Some(SMBIOS_HANDLE_PI_RESERVED), &record)?;

    // Advanced path (explicit control via extension)
    // smbios.add_with_target(None, &mut handle, &record, SmbiosTarget::Force64)?;
    Ok(())
}

// Advanced, low-level escape hatch
pub mod smbios {
    pub mod raw {
        #[repr(C, packed)]
        pub struct SmbiosEntryPoint32 { /* _SM_, checksums, 32-bit table addr, etc. */ }
        #[repr(C, packed)]
        pub struct SmbiosEntryPoint64 { /* _SM3_, checksums, 64-bit table addr, etc. */ }

        /// Build a 32-bit SMBIOS entry-point from a serialized table buffer.
        pub fn build_entry_point32(_table: &[u8]) -> SmbiosEntryPoint32 { unimplemented!() }

        /// Build a 64-bit SMBIOS entry-point from a serialized table buffer.
        pub fn build_entry_point64(_table: &[u8]) -> SmbiosEntryPoint64 { unimplemented!() }

        /// Publish entry-points to the system configuration table (unsafe by nature).
        pub unsafe fn install_config_table_32(_ep: &SmbiosEntryPoint32) -> Result<(), ()> { unimplemented!() }
        pub unsafe fn install_config_table_64(_ep: &SmbiosEntryPoint64) -> Result<(), ()> { unimplemented!() }
    }
}
- Is there value in exposing lower-level table construction functionality to advanced users?
- Should we provide typed interfaces for specific SMBIOS record types (Type 0, Type 1, etc.)?
- Is there value in exposing lower-level table construction functionality to advanced users?
- Should we provide typed interfaces for specific SMBIOS record types (Type 0, Type 1, etc.)?

## Prior Art (Existing PI C Implementation)

This Patina-based SMBIOS implementation follows the SMBIOS protocol
as described in the UEFI specification. *See [Protocols](#protocols) for more information.*

In C, `SMBIOS_INSTANCE` provides the core management structure,
`EFI_SMBIOS_ENTRY` represents individual SMBIOS records,
and `SMBIOS_HANDLE_ENTRY` tracks allocated handles.
These are roughly replicated by the Rust structs described in [SMBIOS Records](#smbios-records).

### Dependencies on C Protocols

While the final outcome should be a purely Rust-based interface,
a Rust implementation of SMBIOS services currently relies on C protocols like `BootServices.InstallConfigurationTable`
to publish SMBIOS tables to the system configuration table.
This must also be eventually reimplemented in Rust to achieve a pure Rust SMBIOS implementation.

## Rust Code Design

### SMBIOS Records Service

Integrated functionality for adding, updating, removing, and retrieving SMBIOS records
will be provided through the `SmbiosRecords` service.

```rust
pub trait SmbiosRecords {
    type Iter: Iterator<Item = &SmbiosRecord>;

    /// Adds an SMBIOS record to the SMBIOS table.
    ///
    /// Per EDK2 usage, if a handle is provided and already in use, EFI_ALREADY_STARTED is returned and the record is not updated.
    /// However, automatic handle assignment is preferred and aligns with idiomatic Rust API design.
    ///
    /// It is recommended to omit the handle argument entirely, letting the implementation always assign a unique handle.
    /// This avoids ambiguity, simplifies usage, and matches the most common use case.
    fn add(
        &mut self,
        producer_handle: Option<Handle>,
        record: &SmbiosTableHeader,
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
    fn iter(&self) -> Self::Iter;

    /// Gets the SMBIOS version information.
    fn version(&self) -> (u8, u8); // (major, minor)
}

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
```

### SMBIOS Records

Core SMBIOS record structures:

```rust
/// SMBIOS table header structure
#[repr(C, packed)]
#[derive(Debug, Clone)]
pub struct SmbiosTableHeader {
    pub record_type: u8,
    pub length: u8,
    pub handle: SmbiosHandle,
    // Variable-length data follows
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

/// BIOS Information (Type 0) - typed interface
#[repr(C, packed)]
pub struct BiosInformation {
    pub header: SmbiosTableHeader,
    pub vendor: u8,              // String index
    pub bios_version: u8,        // String index  
    pub bios_segment: u16,
    pub bios_release_date: u8,   // String index
    pub bios_size: u8,
    pub characteristics: u64,
    pub characteristics_ext1: u8,
    pub characteristics_ext2: u8,
    pub major_release: u8,
    pub minor_release: u8,
    pub ec_major_release: u8,
    pub ec_minor_release: u8,
}

impl BiosInformation {
    pub fn new() -> Self {
        Self {
            header: SmbiosTableHeader {
                record_type: 0, // Type 0
                length: size_of::<BiosInformation>() as u8,
                handle: SMBIOS_HANDLE_PI_RESERVED,
            },
            vendor: 1,           // First string
            bios_version: 2,     // Second string
            bios_segment: 0xE000,
            bios_release_date: 3, // Third string
            bios_size: 0x0F,     // Default size
            characteristics: 0x08, // PCI supported
            characteristics_ext1: 0x01, // ACPI supported
            characteristics_ext2: 0x00,
            major_release: 1,
            minor_release: 0,
            ec_major_release: 0xFF, // Not supported
            ec_minor_release: 0xFF, // Not supported
        }
    }
}
```

### SMBIOS Support Structure

A `SmbiosManager` struct provides the core SMBIOS functionality and maintains the global state:

```rust
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
            std::slice::from_raw_parts(
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

impl SmbiosRecords for SmbiosManager {
    type Iter = std::slice::Iter<'static, SmbiosRecord>;

    fn add(
        &mut self,
        producer_handle: Option<Handle>,
        smbios_handle: &mut SmbiosHandle,
        record: &SmbiosTableHeader,
    ) -> Result<(), SmbiosError> {
        let _lock = self.lock.lock().unwrap();

        // Assign handle if needed
        if *smbios_handle == SMBIOS_HANDLE_PI_RESERVED {
            *smbios_handle = self.allocate_handle()?;
        } else if self.allocated_handles.contains(smbios_handle) {
            return Err(SmbiosError::HandleAlreadyInUse);
        } else {
            self.allocated_handles.insert(*smbios_handle);
        }

        // Create record data (simplified - would need proper string parsing)
        let record_size = record.length as usize;
        let mut data = Vec::with_capacity(record_size + 2); // +2 for double null
        

        unsafe {
            let bytes = std::slice::from_raw_parts(
                record as *const _ as *const u8,
                record_size,
            );

            data.extend_from_slice(bytes);
        }
        
        // Add double null terminator (simplified)
        data.extend_from_slice(&[0, 0]);



        let smbios_record = SmbiosRecord {
            header: *record,
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
        
        let _lock = self.lock.lock().unwrap();
        
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
        let _lock = self.lock.lock().unwrap();
        
        let pos = self.records
            .iter()
            .position(|r| r.header.handle == smbios_handle)
            .ok_or(SmbiosError::NotFound)?;

        self.records.remove(pos);
        self.allocated_handles.remove(&smbios_handle);
        Ok(())
    }

    fn get_next(
        &self,
        smbios_handle: &mut SmbiosHandle,
        record_type: Option<SmbiosType>,
    ) -> Result<(&SmbiosTableHeader, Option<Handle>), SmbiosError> {
        let _lock = self.lock.lock().unwrap();
        
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
        Err(SmbiosError::NotFound)
    }

    fn iter(&self) -> Self::Iter {
        // This is a simplified implementation
        // Real implementation would need proper lifetime management
        unsafe { std::mem::transmute(self.records.iter()) }
    }

    fn version(&self) -> (u8, u8) {
        (self.major_version, self.minor_version)
    }
}
```

### SMBIOS Component

Initialization responsibilities are owned by `SmbiosManager`.

The `SmbiosManager` exposes explicit initialization and registration methods (for example
`init()` and `register()` or a single `init_and_register()` convenience method) which are
responsible for building tables, installing system configuration tables, and publishing the
SMBIOS provider / C protocol. This keeps all SMBIOS logic and state in the manager type and
keeps any component-level bootstrap very small.

Components should construct a `SmbiosManager` and call its init/register methods. A thin
bootstrap component may still exist (for example to wire configuration and dependency injection),
but it should contain no SMBIOS logic beyond creating and handing the manager to the runtime.

#### EDK II Protocol

The global `SmbiosManager` will produce a C protocol to support the existing EDK II `EFI_SMBIOS_PROTOCOL`:

```rust
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

type SmbiosAdd = extern "efiapi" fn(
    *const SmbiosProtocol,
    efi::Handle,
    *mut SmbiosHandle,
    *const SmbiosTableHeader,
) -> efi::Status;

type SmbiosUpdateString = extern "efiapi" fn(
    *const SmbiosProtocol,
    *mut SmbiosHandle,
    *mut usize,
    *const c_char,
) -> efi::Status;

type SmbiosRemove = extern "efiapi" fn(
    *const SmbiosProtocol,
    SmbiosHandle,
) -> efi::Status;

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

        // Enforce automatic handle assignment: do not allow adding to a non-reserved handle
        unsafe {
            if *smbios_handle != SMBIOS_HANDLE_PI_RESERVED {
                // External caller tried to add to a specific handle, which is not allowed
                return efi::Status::ALREADY_STARTED;
            }
        }

        // Example: Use the internal Rust API to add a record
        // (Assume a global SMBIOS manager instance is available)
        let manager = get_global_smbios_manager(); // This function is illustrative

        // SAFETY: smbios_handle and record are checked above
        let mut handle = SMBIOS_HANDLE_PI_RESERVED;
        let header = unsafe { &*record };

        // Call the Rust API
        match manager.lock().unwrap().add(Some(producer_handle), &header) {
            Ok(assigned_handle) => {
                unsafe { *smbios_handle = assigned_handle; }
                efi::Status::SUCCESS
            }
            Err(SmbiosError::InvalidParameter) => efi::Status::INVALID_PARAMETER,
            Err(SmbiosError::OutOfResources) => efi::Status::OUT_OF_RESOURCES,
            Err(SmbiosError::HandleAlreadyInUse) => efi::Status::ALREADY_STARTED,
            Err(_) => efi::Status::DEVICE_ERROR,
        }
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

    extern "efiapi" fn remove_ext(
        _protocol: *const SmbiosProtocol,
        smbios_handle: SmbiosHandle,
    ) -> efi::Status {
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
```

## Guide-Level Explanation

All interaction with SMBIOS records will be mediated by the `SmbiosRecords` trait interface.
Consumers will access this service as follows:

```rust
pub fn component(smbios_records: Service<dyn SmbiosRecords>) -> Result<()> {
    let handle = smbios_records.add(None, None, &record)?;
    
    // Update a string in the record
    smbios_records.update_string(handle, 1, "New String Value")?;
    
    // Iterate through all records
    for record in smbios_records.iter() {
        // Process each record
    }
    
        // Remove the record
    smbios_records.remove(handle)?;
    Ok(())
}
```

When adding a record with `add`, automatic handle assignment is always used; the API does not allow adding a record to an
existing handle. This ensures unique handle allocation and avoids ambiguity or errors from reusing handles.

The service automatically handles:

- SMBIOS table construction and checksums
- Handle allocation and deduplication
- String parsing and validation
- Memory management for both 32-bit and 64-bit tables
- Installation of system configuration tables

### BIOS Information Example (Type 0)

Below is an example of creating and installing a BIOS Information (Type 0) SMBIOS record:

```rust
use std::mem::size_of;

// Configuration for BIOS info
struct BiosInfoConfig {
    vendor: String,
    version: String,
    release_date: String,
    major_release: u8,
    minor_release: u8,
}

#[derive(IntoComponent)]
struct SmbiosBiosInfoManager;

impl SmbiosBiosInfoManager {
    fn entry_point(
        self,
        config: Config<BiosInfoConfig>,
        smbios_records: Service<dyn SmbiosRecords>,
    ) -> patina_sdk::error::Result<()> {
        let mut bios_info = BiosInformation::new();
        bios_info.major_release = config.major_release;
        bios_info.minor_release = config.minor_release;

        // Build the complete record with strings
        let strings = vec![
            config.vendor.as_str(),
            config.version.as_str(),
            config.release_date.as_str(),
        ];

        let record_data = SmbiosManager::build_record_with_strings(
            &bios_info.header,
            &strings,
        )?;

        let mut handle = SMBIOS_HANDLE_PI_RESERVED;
        smbios_records.add(
            None,
            &mut handle,
            unsafe { &*(record_data.as_ptr() as *const SmbiosTableHeader) },
        )?;

        log::info!("Added BIOS Information record with handle: {}", handle);
        Ok(())
    }
}

#[derive(IntoComponent)]
struct SmbiosRecordsManager;

impl SmbiosRecordsManager {
    fn entry_point(self) -> patina_sdk::error::Result<()> {
        let smbios_manager = SmbiosManager::new(3, 0); // SMBIOS 3.0
        smbios_manager.register(); // Brings `SmbiosRecords` service up
        Ok(())
    }
}

fn _start() {
    let bios_config = BiosInfoConfig {
        vendor: "Example Corp".to_string(),
        version: "1.0.0".to_string(),
        release_date: "07/24/2025".to_string(),
        major_release: 1,
        minor_release: 0,
    };

    Core::default()
        .with_component(SmbiosRecordsManager) // Initialize SMBIOS service
        .with_config(bios_config)
        .with_component(SmbiosBiosInfoManager) // Add BIOS info record
        // ... other components
        .run()
}
```

This example demonstrates:

- Creating a typed SMBIOS record structure
- Building the complete record with strings
- Using the SmbiosRecords service to install the record
- Automatic handle assignment
- String management and null termination

## Testing Strategy

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_bios_info_record_creation() {
        let mut manager = SmbiosManager::new(3, 0);
        let mut handle = SMBIOS_HANDLE_PI_RESERVED;
        
        // Create BIOS info record
        let bios_info = BiosInformation::new();
        
        let strings = vec!["Test Vendor", "1.0.0", "07/24/2025"];
        let record_data = SmbiosManager::build_record_with_strings(
            &bios_info.header, 
            &strings
        ).unwrap();
        
    let result = manager.add(
            None,
            &mut handle,
            unsafe { &*(record_data.as_ptr() as *const SmbiosTableHeader) }
        );
        
        assert!(result.is_ok());
        assert_ne!(handle, SMBIOS_HANDLE_PI_RESERVED);
    }
    
    #[test]
    fn test_string_validation() {
        // Test normal string
        assert!(SmbiosManager::validate_string("Normal String").is_ok());
        
        // Test too long string
        let long_string = "a".repeat(65);
        assert_eq!(
            SmbiosManager::validate_string(&long_string),
            Err(SmbiosError::StringTooLong)
        );
        
        // Test string with null
        assert_eq!(
            SmbiosManager::validate_string("Bad\0String"),
            Err(SmbiosError::InvalidParameter)
        );
    }

    #[test]
    fn test_handle_allocation() {
        let mut manager = SmbiosManager::new(3, 0);
        
        let handle1 = manager.allocate_handle().unwrap();
        let handle2 = manager.allocate_handle().unwrap();
        
        assert_ne!(handle1, handle2);
        assert!(handle1 > 0 && handle1 < 0xFF00);
        assert!(handle2 > 0 && handle2 < 0xFF00);
    }

    #[test]
    fn test_duplicate_handle_rejection() {
        let mut manager = SmbiosManager::new(3, 0);
        let mut handle = 100;
        
        // First add should succeed
        let bios_info = BiosInformation::new();
        let strings = vec!["Vendor", "Version", "Date"];
        let record_data = SmbiosManager::build_record_with_strings(
            &bios_info.header, 
            &strings
        ).unwrap();
        
    let result1 = manager.add(
            None,
            &mut handle,
            unsafe { &*(record_data.as_ptr() as *const SmbiosTableHeader) }
        );
        assert!(result1.is_ok());
        
        // Second add with same handle should fail
        let mut duplicate_handle = 100;
    let result2 = manager.add(
            None,
            &mut duplicate_handle,
            unsafe { &*(record_data.as_ptr() as *const SmbiosTableHeader) }
        );
        assert_eq!(result2, Err(SmbiosError::AlreadyStarted));
    }
}
```

## Migration Strategy

### Phase 1: Rust Implementation with C Compatibility

- Implement the Rust SMBIOS provider service
- Create C-compatible protocol interfaces
- Test with existing EDK2 components

### Phase 2: Component Migration

- Migrate individual SMBIOS record producers to use Rust service
- Replace C-based SMBIOS table construction with Rust implementation
- Maintain backward compatibility during transition

### Phase 3: Pure Rust Implementation

- Remove C protocol dependencies
- Implement pure Rust configuration table installation
- Complete migration of all SMBIOS-related components

## Performance Considerations

### Memory Efficiency

- Rust's zero-cost abstractions provide equivalent performance to C
- Smart pointer usage minimizes memory allocation overhead
- Compile-time optimization of record structure layouts

### Runtime Performance

- Handle allocation uses efficient hash set lookups
- String validation occurs only once during record creation
- Lazy evaluation for record iteration and filtering

### Memory Safety Benefits

- Eliminates buffer overflow vulnerabilities common in C implementations
- Type-safe handle management prevents use-after-free errors
- Automatic memory management reduces memory leaks

## Compatibility Matrix

| SMBIOS Version | Support Level | Notes |
|----------------|---------------|-------|
| 2.0 - 2.7      | Full          | 32-bit entry point, legacy compatibility |
| 3.0 - 3.8      | Full          | 64-bit entry point, modern systems |
| Future versions| Extensible    | Design supports future specification updates |

| UEFI Version   | Support Level | Notes |
|----------------|---------------|-------|
| 2.0 - 2.4      | Full          | Basic protocol compatibility |
| 2.5 - 2.10     | Full          | Enhanced features supported |
| Future versions| Forward compatible | Rust design enables easy updates |

## Security Considerations

### Input Validation

- All string inputs validated for length and content
- Handle values checked for validity and uniqueness
- Record structures validated for proper formatting

### Memory Safety

- No unsafe memory operations in public API
- Bounds checking on all array accesses
- Protection against integer overflow in size calculations

### Privilege Separation

- Producer handles tracked separately from SMBIOS handles
- Access control for record modification operations
- Audit trail for record lifecycle events

## Future Extensions

### Typed Record Interfaces

Consider adding strongly-typed interfaces for common SMBIOS record types:

```rust
/// System Information (Type 1)
#[repr(C, packed)]
pub struct SystemInformation {
    pub header: SmbiosTableHeader,
    pub manufacturer: u8,        // String index
    pub product_name: u8,        // String index
    pub version: u8,             // String index
    pub serial_number: u8,       // String index
    pub uuid: [u8; 16],
    pub wake_up_type: u8,
    pub sku_number: u8,          // String index
    pub family: u8,              // String index
}

/// Baseboard Information (Type 2)
#[repr(C, packed)]
pub struct BaseboardInformation {
    pub header: SmbiosTableHeader,
    pub manufacturer: u8,        // String index
    pub product: u8,             // String index
    pub version: u8,             // String index
    pub serial_number: u8,       // String index
    pub asset_tag: u8,           // String index
    pub feature_flags: u8,
    pub location_in_chassis: u8, // String index
    pub chassis_handle: u16,
    pub board_type: u8,
    pub contained_object_handles: u8,
}
```

### Dynamic Record Builder

Provide a builder pattern for complex record construction:

```rust
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
            std::slice::from_raw_parts(
                &value as *const T as *const u8,
                std::mem::size_of::<T>(),
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
            length: (std::mem::size_of::<SmbiosTableHeader>() + self.data.len()) as u8,
            handle: SMBIOS_HANDLE_PI_RESERVED,
        };
        
        let header_bytes = unsafe {
            std::slice::from_raw_parts(
                &header as *const _ as *const u8,
                std::mem::size_of::<SmbiosTableHeader>(),
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

// Usage example:
let record = SmbiosRecordBuilder::new(1) // System Information
    .add_field(1u8)  // manufacturer string index
    .add_field(2u8)  // product name string index
    .add_string("ACME Corp".to_string())?
    .add_string("SuperServer 3000".to_string())?
    .build()?;
```

### Configuration-Driven Record Generation

Enable platform-specific SMBIOS table generation through configuration:

```rust
#[derive(Deserialize)]
pub struct SmbiosConfig {
    pub bios_info: BiosInfoConfig,
    pub system_info: SystemInfoConfig,
    pub baseboard_info: BaseboardInfoConfig,
    pub custom_records: Vec<CustomRecordConfig>,
}

#[derive(Deserialize)]
pub struct CustomRecordConfig {
    pub record_type: u8,
    pub fields: Vec<FieldConfig>,
    pub strings: Vec<String>,
}

impl SmbiosManager {
    pub fn from_config(config: &SmbiosConfig) -> Result<Self, SmbiosError> {
        let mut manager = Self::new(3, 0);
        
        // Generate standard records
        manager.add_bios_info(&config.bios_info)?;
        manager.add_system_info(&config.system_info)?;
        manager.add_baseboard_info(&config.baseboard_info)?;
        
        // Generate custom records
        for custom in &config.custom_records {
            manager.add_custom_record(custom)?;
        }
        
        Ok(manager)
    }
}
```
