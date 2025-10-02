# RFC 0018: `SMBIOS`

This RFC proposes a Rust-based interface for SMBIOS table management, providing the functionality described in the
`EFI_SMBIOS_PROTOCOL` service defined in the UEFI Specification.

It introduces an `SmbiosRecords` trait to manage addition, removal, updates, and retrieval of SMBIOS records,
encapsulating the SMBIOS protocol behind a safe, idiomatic Rust API.

The goal is to maintain compatibility with the UEFI specification while improving memory safety and simplifying
integration for components.

## Change Log

- 2025-07-28: Initial RFC created.
- 2025-09-16: Updated `add` method signature to return handle and marked unsafe to address memory safety concerns.
  Added safe `add_from_bytes` alternative.
- 2025-09-18: Revised to support only SMBIOS 3.0+ with 64-bit entry point structures for improved UEFI compatibility
  and simplified architecture. Removed 32-bit format support and related API complexity. Removed low-level construction
  functionality based on security feedback to ensure specification compliance and prevent malformed SMBIOS structures.
- 2025-10-02: Updated RFC to reflect actual implementation status. Moved advanced versioned record interfaces to Future
  Work section. Added documentation for component/service pattern, counter-based handle allocation, and comprehensive
  string validation features that are implemented.

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

This implementation supports SMBIOS 3.0+ which uses 64-bit entry point structures,
allowing the UEFI configuration table to point to entry point structures anywhere in
addressable space and enabling all structures to reside in memory above 4GB.
This differs from SMBIOS 2.x which required entry point structures to be paragraph
aligned in Segment 0xF000 (below 1MB) with structure arrays limited to RAM below 4GB.

Since SMBIOS 3.0 is designed specifically to support UEFI environments and can be
configured to support legacy constraints when needed, this implementation focuses
exclusively on SMBIOS 3.0+ to provide the most robust and future-compatible solution.

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
4. Support SMBIOS 3.0+ (64-bit) table format exclusively for maximum UEFI compatibility and future-proofing
5. Provide safe string manipulation for SMBIOS records

## Rationale for SMBIOS 3.0+ Only Support

This implementation supports exclusively SMBIOS version 3.0 and later for the following reasons:

**UEFI Compatibility**: SMBIOS 3.0+ was specifically designed to support UEFI environments. The 64-bit entry
point structure allows the UEFI configuration table to point to entry point structures anywhere in addressable
memory space, removing the constraints of legacy BIOS environments.

**Memory Layout Flexibility**: Unlike SMBIOS 2.x which requires:

- Entry point structures to be paragraph-aligned in Segment 0xF000 (below 1MB)
- Structure arrays limited to RAM below 4GB

SMBIOS 3.0+ enables:

- Entry point structures anywhere in addressable space
- All structures to reside in memory above 4GB
- Full utilization of modern system memory layouts

**Backward Compatibility**: SMBIOS 3.0 can be configured to support the 1MB and 4GB constraints when required
for legacy compatibility, making it a superset of SMBIOS 2.x capabilities.

**Future-Proofing**: By focusing on the modern specification, this implementation avoids the complexity of
supporting multiple entry point formats while providing the most robust foundation for future enhancements.

**Simplified Architecture**: Supporting only the 64-bit entry point structure eliminates the need for dual
code paths and reduces the potential for format-specific bugs, resulting in a cleaner and more maintainable
implementation.

## Memory Safety Considerations

**Security-First Approach**: This implementation prioritizes memory safety and specification compliance. While some
methods are marked as `unsafe` to maintain compatibility with existing UEFI protocols, **the safe alternatives should be
strongly preferred** for all new implementations.

### Unsafe Methods (Use with Extreme Caution)

The following methods are provided only for compatibility with existing UEFI protocol patterns:

- **`add()`**: Takes a `&SmbiosTableHeader` but assumes it points to a complete SMBIOS record with the structured data
  following the header in memory. This is dangerous because a caller could pass just a header struct without the actual
  record data, leading to buffer overruns and potential security vulnerabilities.

- **`build_record_with_strings()`**: Similar issue - assumes the header parameter points to the complete structured
  portion of the record.

### Recommended Safe Alternatives

**Always prefer these methods for new code:**

- **`add_from_bytes()`**: Takes the complete record as a byte slice, avoiding unsafe pointer arithmetic and ensuring
  specification compliance
- Create complete record data as validated byte vectors before passing to the API
- Use structured builders that validate SMBIOS record format during construction

### Security Impact of Corrupted Headers

**Critical Vulnerability**: If external code can pass in a `SmbiosTableHeader` with a corrupted `length` field,
there is no safe way to parse the record without risking buffer overruns and memory corruption.

**Example of the dangerous pattern this design avoids:**

```rust
// DANGEROUS: External header construction with potentially corrupted length
let bogus_header = SmbiosTableHeader { length: 0x1234, /* other fields */ };
let record_data = unsafe { 
    SmbiosManager::build_record_with_strings(&bogus_header, strings) 
}; // Could read beyond valid memory if length field is corrupted
```

### Safe Design Solution

**This implementation eliminates the vulnerability by ensuring the SMBIOS service constructs all headers internally:**

```rust
// SAFE: Service-controlled header construction
let mut record_bytes = Vec::new();
// Application provides only structured data and string pool as validated bytes
record_bytes.extend_from_slice(&structured_data_bytes);
record_bytes.extend_from_slice(&string_pool_bytes);

// Service validates data and constructs header with correct length field
let handle = smbios.add_from_bytes(None, &record_bytes)?;
```

**Security Guarantees:**

1. **Service-controlled headers**: All `SmbiosTableHeader` instances are constructed by the trusted SMBIOS service
2. **Length validation**: The service calculates the length field based on actual validated data
3. **No external header input**: Applications cannot provide potentially corrupted headers
4. **Complete buffer validation**: All parsing operations are bounds-checked against provided buffers

## Design Decisions

### SMBIOS Version Support

**Decision**: Support only SMBIOS 3.0+ with 64-bit entry point structures.

**Rationale**: SMBIOS 3.0+ was designed specifically for UEFI environments and provides superior memory layout
flexibility while maintaining backward compatibility through configuration. This eliminates the complexity of dual
format support while providing the most robust foundation for modern firmware implementations.

**Impact**: The API provides a unified interface without the need for format-specific code paths or version
selection. All records use the modern 64-bit addressing model, simplifying both implementation and usage.

### No Low-Level Construction Access

**Decision**: This implementation will NOT expose lower-level table construction functionality to advanced users.

**Rationale**: The SMBIOS entry point structure is only 24 bytes long and contains specific, standardized data including:

- Spec version supported
- Table address
- Other specification-defined fields

The SMBIOS tables themselves follow a clearly defined structure:

- 4-byte header indicating the length of fixed-size values
- Double NULL-terminated string pool following the specification

**Security Considerations**: Providing access to low-level construction could compromise system security by allowing:

- Malformed entry point structures
- Invalid table headers
- Corrupted string pools
- Non-compliant SMBIOS data structures

**Flexibility for OEMs**: If an OEM needs to diverge from the SMBIOS specification for specific requirements, they can
create a local override of this crate rather than compromising the security of the standard implementation.

**Alternative**: The safe `add_from_bytes()` method provides sufficient flexibility for adding compliant SMBIOS records
while maintaining specification adherence and memory safety.

**Note on Remaining Unsafe Methods**: The `unsafe fn add()` method remains in the trait definition solely for
compatibility with existing UEFI protocol patterns. However, **it should be strongly discouraged in favor of the safe
`add_from_bytes()` alternative** for all new implementations. The unsafe method exists only to support legacy code
migration scenarios.

## Current Implementation Status

### Implemented Features

The current implementation provides a complete, production-ready SMBIOS service with the following features:

#### Core Service Architecture

**Component Pattern**: The SMBIOS implementation follows Patina's component/service pattern:

```rust
#[derive(IntoComponent, IntoService)]
#[service(dyn SmbiosRecords<'static>)]
pub struct SmbiosProviderManager {
    manager: SmbiosManager,
}
```

**Service Registration**: The service is registered using the Commands pattern in the entry point:

```rust
fn entry_point(
    mut self,
    config: Option<Config<SmbiosConfiguration>>,
    mut commands: Commands,
) -> Result<()> {
    // Configure SMBIOS version
    let cfg = config.map(|c| (*c).clone()).unwrap_or_default();
    self.manager = SmbiosManager::new(cfg.major_version, cfg.minor_version);
    
    // Register service for consumption by other components
    commands.add_service(self);
    Ok(())
}
```

**Configuration**: Platforms can configure the SMBIOS version:

```rust
pub struct SmbiosConfiguration {
    pub major_version: u8,  // Defaults to 3
    pub minor_version: u8,  // Defaults to 0
}
```

#### Handle Allocation

**Counter-Based Algorithm**: Implements an optimized counter-based handle allocation strategy:

- **O(1) average case** performance vs HashSet lookup overhead
- **Sequential allocation** from `next_handle` counter (starts at 1)
- **Wraparound logic** from 0xFEFF back to 1 when exhausted
- **Skip reserved handles** (0, 0xFFFE, 0xFFFF)
- **Reuse optimization** in `remove()` - resets counter to freed handle if lower

```rust
fn allocate_handle(&mut self) -> Result<SmbiosHandle, SmbiosError> {
    let mut attempts = 0u32;
    const MAX_ATTEMPTS: u32 = 0xFEFF;

    loop {
        let candidate = self.next_handle;
        
        // Skip reserved handles
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

        if attempts >= MAX_ATTEMPTS {
            return Err(SmbiosError::OutOfResources);
        }
    }
}
```

#### String Validation

**Comprehensive String Validation**: Multiple layers of string validation ensure SMBIOS compliance:

1. **Individual String Validation** - `validate_string()`:
   - Maximum length check (64 bytes per SMBIOS spec)
   - Null byte prohibition (strings are null-terminated)

2. **String Pool Validation** - `validate_and_count_strings()`:
   - Single-pass O(n) algorithm
   - Validates double-null termination
   - Counts strings while validating
   - Detects empty vs non-empty pools
   - Per-string length enforcement

3. **Complete Record Validation** - `count_strings_in_record()`:
   - Validates minimum buffer size
   - Extracts and validates header length
   - Ensures string pool has required space
   - Delegates to string pool validator

#### Safe API

**Primary Safe Interface** - `add_from_bytes()`:

The recommended API for adding SMBIOS records performs comprehensive validation:

```rust
fn add_from_bytes(
    &mut self,
    producer_handle: Option<Handle>,
    record_data: &[u8],
) -> Result<SmbiosHandle, SmbiosError> {
    // Step 1: Validate minimum size for header
    // Step 2: Parse and validate header
    // Step 3: Validate header->length vs buffer size
    // Step 4: Extract string pool area
    // Step 5: Validate string pool format and count strings
    // If all validation passes, allocate handle and store record
}
```

**Unsafe Legacy API** - `add()`:

Retained for UEFI protocol compatibility but strongly discouraged. Requires caller to guarantee:

- Complete SMBIOS record structure
- Valid memory at pointer location
- Proper string pool formatting

#### String Updates

**Complete String Update Implementation** - `update_string()`:

Full implementation that parses, updates, and rebuilds record data:

1. Validates string format and index bounds
2. Parses existing string pool from record data
3. Extracts all current null-terminated strings
4. Replaces the target string (1-indexed per SMBIOS spec)
5. Rebuilds complete record with:
   - Original structured header data
   - Updated string pool
   - Proper null terminators

#### Thread Safety

**Mutex Protection**: All mutable operations are protected by a spin::Mutex:

```rust
fn update_string(...) -> Result<(), SmbiosError> {
    let _lock = self.lock.lock();  // Guard held for operation duration
    // ... operation ...
}
```

#### Trait Object Safety

**Service Compatibility**: The `SmbiosRecords` trait is designed for use as a trait object:

- Returns `Box<dyn Iterator>` instead of associated type for `iter()`
- Enables `dyn SmbiosRecords<'static>` for service registration
- Compatible with Patina's service injection system

### Not Yet Implemented

The following features are documented in this RFC but not yet implemented:

- Versioned typed record interfaces (`VersionedSmbiosRecord` trait)
- Typed record methods (`add_typed_record()`, `get_typed_record()`, `iter_typed_records()`)
- Forward/backward compatibility with version-aware parsing
- Field layout introspection and generic serialization
- SMBIOS protocol FFI bindings (C ABI extern functions)
- Installation of SMBIOS configuration tables

These features are described below as future enhancements.

## Future Work: Versioned Typed Record Interfaces

> **Note**: The features described in this section represent future planned enhancements and are not currently implemented.
> The current implementation provides a complete byte-based API that is production-ready. These typed interfaces would
> provide additional convenience and type safety for specific use cases.

### Design Philosophy

A versioned approach to typed SMBIOS record interfaces would address the challenge of
specification evolution while maintaining forward and backward compatibility. Each SMBIOS record type could have
multiple versions corresponding to different SMBIOS specification releases.

### Key Design Principles

1. **Version Awareness**: Each typed record structure includes version information from the SMBIOS specification
2. **Forward Compatibility**: Implementations gracefully handle records with more data than expected
3. **Backward Compatibility**: Newer implementations can work with older record formats
4. **Safe Parsing**: Unknown or newer fields are preserved but not interpreted
5. **Extensible Design**: New specification versions can be added without breaking existing code

### Versioned Record Trait

```rust
/// Trait for versioned SMBIOS record types
pub trait VersionedSmbiosRecord: Sized {
    /// The SMBIOS record type number
    const RECORD_TYPE: u8;
    
    /// The minimum SMBIOS specification version that introduced this record format
    const MIN_SPEC_VERSION: (u8, u8);
    
    /// Parse a record from raw bytes, handling version differences
    fn from_bytes(data: &[u8], spec_version: (u8, u8)) -> Result<Self, SmbiosError>;
    
    /// Convert the record back to raw bytes
    fn to_bytes(&self) -> Vec<u8>;
    
    /// Get the specification version this record was parsed with
    fn parsed_version(&self) -> (u8, u8);
    
    /// Check if this record format supports a given specification version
    fn supports_version(spec_version: (u8, u8)) -> bool {
        spec_version >= Self::MIN_SPEC_VERSION
    }
}
```

### Version-Aware BIOS Information (Type 0)

```rust
/// BIOS Information (Type 0) with version-aware parsing
pub struct BiosInformation {
    pub header: SmbiosTableHeader,
    // Fields present in SMBIOS 2.0+
    pub vendor: u8,              // String index
    pub bios_version: u8,        // String index  
    pub bios_segment: u16,
    pub bios_release_date: u8,   // String index
    pub bios_size: u8,
    pub characteristics: u64,
    
    // Fields added in SMBIOS 2.4+
    pub characteristics_ext1: Option<u8>,
    pub characteristics_ext2: Option<u8>,
    
    // Fields added in SMBIOS 2.4+
    pub major_release: Option<u8>,
    pub minor_release: Option<u8>,
    
    // Fields added in SMBIOS 2.4+
    pub ec_major_release: Option<u8>,
    pub ec_minor_release: Option<u8>,
    
    // Fields added in SMBIOS 3.1+
    pub extended_bios_size: Option<u16>,
    
    // Track which version this was parsed with
    parsed_version: (u8, u8),
    
    // Preserve unknown future fields
    unknown_data: Vec<u8>,
}

impl VersionedSmbiosRecord for BiosInformation {
    const RECORD_TYPE: u8 = 0;
    const MIN_SPEC_VERSION: (u8, u8) = (2, 0);
    
    fn from_bytes(data: &[u8], spec_version: (u8, u8)) -> Result<Self, SmbiosError> {
        if data.len() < core::mem::size_of::<SmbiosTableHeader>() {
            return Err(SmbiosError::BufferTooSmall);
        }
        
        let header = unsafe { &*(data.as_ptr() as *const SmbiosTableHeader) };
        
        if header.record_type != Self::RECORD_TYPE {
            return Err(SmbiosError::InvalidParameter);
        }
        
        let mut record = BiosInformation {
            header: *header,
            vendor: 0,
            bios_version: 0,
            bios_segment: 0,
            bios_release_date: 0,
            bios_size: 0,
            characteristics: 0,
            characteristics_ext1: None,
            characteristics_ext2: None,
            major_release: None,
            minor_release: None,
            ec_major_release: None,
            ec_minor_release: None,
            extended_bios_size: None,
            parsed_version: spec_version,
            unknown_data: Vec::new(),
        };
        
        let mut offset = core::mem::size_of::<SmbiosTableHeader>();
        
        // Parse based on available data and spec version
        if data.len() > offset && offset < header.length as usize {
            record.vendor = data[offset];
            offset += 1;
        }
        
        if data.len() > offset && offset < header.length as usize {
            record.bios_version = data[offset];
            offset += 1;
        }
        
        if data.len() > offset + 1 && offset < header.length as usize {
            record.bios_segment = u16::from_le_bytes([data[offset], data[offset + 1]]);
            offset += 2;
        }
        
        if data.len() > offset && offset < header.length as usize {
            record.bios_release_date = data[offset];
            offset += 1;
        }
        
        if data.len() > offset && offset < header.length as usize {
            record.bios_size = data[offset];
            offset += 1;
        }
        
        if data.len() > offset + 7 && offset < header.length as usize {
            record.characteristics = u64::from_le_bytes([
                data[offset], data[offset + 1], data[offset + 2], data[offset + 3],
                data[offset + 4], data[offset + 5], data[offset + 6], data[offset + 7],
            ]);
            offset += 8;
        }
        
        // Fields added in SMBIOS 2.4+
        if spec_version >= (2, 4) && data.len() > offset && offset < header.length as usize {
            record.characteristics_ext1 = Some(data[offset]);
            offset += 1;
        }
        
        if spec_version >= (2, 4) && data.len() > offset && offset < header.length as usize {
            record.characteristics_ext2 = Some(data[offset]);
            offset += 1;
        }
        
        if spec_version >= (2, 4) && data.len() > offset && offset < header.length as usize {
            record.major_release = Some(data[offset]);
            offset += 1;
        }
        
        if spec_version >= (2, 4) && data.len() > offset && offset < header.length as usize {
            record.minor_release = Some(data[offset]);
            offset += 1;
        }
        
        if spec_version >= (2, 4) && data.len() > offset && offset < header.length as usize {
            record.ec_major_release = Some(data[offset]);
            offset += 1;
        }
        
        if spec_version >= (2, 4) && data.len() > offset && offset < header.length as usize {
            record.ec_minor_release = Some(data[offset]);
            offset += 1;
        }
        
        // Fields added in SMBIOS 3.1+
        if spec_version >= (3, 1) && data.len() > offset + 1 && offset < header.length as usize {
            record.extended_bios_size = Some(u16::from_le_bytes([data[offset], data[offset + 1]]));
            offset += 2;
        }
        
        // Handle forward compatibility: preserve unknown future fields
        if offset < header.length as usize && offset < data.len() {
            let remaining_structured_data = core::cmp::min(
                header.length as usize - offset,
                data.len() - offset
            );
            record.unknown_data = data[offset..offset + remaining_structured_data].to_vec();
        }
        
        Ok(record)
    }
    
    fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        
        // Add header (will be updated with correct length later)
        let header_bytes = unsafe {
            core::slice::from_raw_parts(
                &self.header as *const _ as *const u8,
                core::mem::size_of::<SmbiosTableHeader>(),
            )
        };
        bytes.extend_from_slice(header_bytes);
        
        // Add mandatory fields
        bytes.push(self.vendor);
        bytes.push(self.bios_version);
        bytes.extend_from_slice(&self.bios_segment.to_le_bytes());
        bytes.push(self.bios_release_date);
        bytes.push(self.bios_size);
        bytes.extend_from_slice(&self.characteristics.to_le_bytes());
        
        // Add version-specific fields
        if let Some(val) = self.characteristics_ext1 {
            bytes.push(val);
        }
        if let Some(val) = self.characteristics_ext2 {
            bytes.push(val);
        }
        if let Some(val) = self.major_release {
            bytes.push(val);
        }
        if let Some(val) = self.minor_release {
            bytes.push(val);
        }
        if let Some(val) = self.ec_major_release {
            bytes.push(val);
        }
        if let Some(val) = self.ec_minor_release {
            bytes.push(val);
        }
        if let Some(val) = self.extended_bios_size {
            bytes.extend_from_slice(&val.to_le_bytes());
        }
        
        // Add preserved unknown data for forward compatibility
        bytes.extend_from_slice(&self.unknown_data);
        
        // Update header with correct length
        let correct_length = bytes.len() as u8;
        bytes[1] = correct_length; // length field is at offset 1 in SmbiosTableHeader
        
        bytes
    }
    
    fn parsed_version(&self) -> (u8, u8) {
        self.parsed_version
    }
}

impl BiosInformation {
    /// Create a new BIOS Information record for a specific SMBIOS version
    pub fn new_for_version(spec_version: (u8, u8)) -> Self {
        let mut record = Self {
            header: SmbiosTableHeader {
                record_type: 0,
                length: 0, // Will be calculated in to_bytes()
                handle: SMBIOS_HANDLE_PI_RESERVED,
            },
            vendor: 1,
            bios_version: 2,
            bios_segment: 0xE000,
            bios_release_date: 3,
            bios_size: 0x0F,
            characteristics: 0x08,
            characteristics_ext1: None,
            characteristics_ext2: None,
            major_release: None,
            minor_release: None,
            ec_major_release: None,
            ec_minor_release: None,
            extended_bios_size: None,
            parsed_version: spec_version,
            unknown_data: Vec::new(),
        };
        
        // Set version-appropriate defaults
        if spec_version >= (2, 4) {
            record.characteristics_ext1 = Some(0x01); // ACPI supported
            record.characteristics_ext2 = Some(0x00);
            record.major_release = Some(1);
            record.minor_release = Some(0);
            record.ec_major_release = Some(0xFF); // Not supported
            record.ec_minor_release = Some(0xFF); // Not supported
        }
        
        if spec_version >= (3, 1) {
            record.extended_bios_size = Some(0x0000);
        }
        
        record
    }
}
```

### Enhanced SmbiosRecords Service with Typed Support

```rust
pub trait SmbiosRecords {
    // ... existing methods ...
    
    /// Add a versioned typed record
    fn add_typed_record<T: VersionedSmbiosRecord>(
        &mut self,
        producer_handle: Option<Handle>,
        record: &T,
    ) -> Result<SmbiosHandle, SmbiosError> {
        let record_bytes = record.to_bytes();
        self.add_from_bytes(producer_handle, &record_bytes)
    }
    
    /// Get a typed record by handle
    fn get_typed_record<T: VersionedSmbiosRecord>(
        &self,
        smbios_handle: SmbiosHandle,
    ) -> Result<T, SmbiosError> {
        // Find the record
        let record = self.records
            .iter()
            .find(|r| r.header.handle == smbios_handle)
            .ok_or(SmbiosError::HandleNotFound)?;
        
        if record.header.record_type != T::RECORD_TYPE {
            return Err(SmbiosError::UnsupportedRecordType);
        }
        
        T::from_bytes(&record.data, self.version())
    }
    
    /// Iterate over typed records of a specific type
    fn iter_typed_records<T: VersionedSmbiosRecord>(&self) -> impl Iterator<Item = Result<T, SmbiosError>> {
        self.records
            .iter()
            .filter(|r| r.header.record_type == T::RECORD_TYPE)
            .map(|r| T::from_bytes(&r.data, self.version()))
    }
}
```

### Forward Compatibility Example

```rust
// Example: An SMBIOS 3.0 implementation encountering a SMBIOS 3.2 Type 0 record
let smbios_3_2_bios_data = vec![
    // Standard SMBIOS 3.0 Type 0 fields...
    0x00, 0x20, 0x01, 0x00,  // Header: Type 0, Length 32, Handle 1
    0x01, 0x02, 0x00, 0xE0,  // Vendor, Version, Segment
    0x03, 0x0F,              // Date, Size
    0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // Characteristics
    0x01, 0x00,              // Extension bytes (SMBIOS 2.4+)
    0x01, 0x00,              // Major/Minor release
    0xFF, 0xFF,              // EC releases
    0x00, 0x00,              // Extended BIOS size (SMBIOS 3.1+)
    // NEW: Future SMBIOS 3.2 fields that our implementation doesn't know about
    0xAB, 0xCD, 0xEF, 0x12,  // Unknown future fields
];

// Our SMBIOS 3.0 implementation can still parse this safely
let bios_info = BiosInformation::from_bytes(&smbios_3_2_bios_data, (3, 0))?;

// Known fields are parsed correctly
assert_eq!(bios_info.vendor, 1);
assert_eq!(bios_info.major_release, Some(1));

// Unknown future fields are preserved
assert_eq!(bios_info.unknown_data, vec![0xAB, 0xCD, 0xEF, 0x12]);

// When converting back to bytes, the unknown data is preserved
let preserved_bytes = bios_info.to_bytes();
assert!(preserved_bytes.ends_with(&[0xAB, 0xCD, 0xEF, 0x12]));
```

### Migration Strategy for Typed Records

1. **Gradual Adoption**: Existing byte-based APIs remain available
2. **Opt-in Typing**: Components can choose to use typed interfaces when needed
3. **Version Detection**: Automatic detection of appropriate record versions based on SMBIOS spec version
4. **Compatibility Testing**: Comprehensive tests ensure new versions don't break existing functionality

This approach would address version compatibility by providing:

- **Version Awareness**: Each record type can handle multiple SMBIOS specification versions
- **Forward Compatibility**: Unknown fields are preserved, allowing older implementations to work with newer records
- **Backward Compatibility**: Newer implementations can gracefully handle older record formats
- **Safety**: All parsing is bounds-checked and error-handling is explicit

## Future Work: Sized Structures with Byte Array Interface (Alternative Approach)

> **Note**: This section describes an alternative design approach for future consideration. Neither this approach nor
> the versioned typed interfaces above are currently implemented.

### Alternative Design Philosophy

An alternative to the versioned typed interfaces above would be to provide sized structures for SMBIOS records
while maintaining a simple byte array interface at the service level. This approach would offer a middle ground
between type safety and simplicity.

### Alternative Design Principles

1. **Simple Service Interface**: The core service API remains byte-array based for maximum flexibility
2. **Structured Record Definitions**: Provide well-defined structures for standard SMBIOS record types
3. **OEM Extensibility**: Enable OEMs to define custom record structures using the same pattern
4. **Validation Focus**: Emphasize validation at the service boundary rather than complex type systems
5. **String Pool Integration**: Include string management directly in the record structures

### Scalable Record Structure Pattern

Approach that uses generic serialization:

```rust
/// Base trait for SMBIOS record structures with generic serialization
pub trait SmbiosRecordStructure {
    /// The SMBIOS record type number
    const RECORD_TYPE: u8;
    
    /// Convert the structure to a complete SMBIOS record byte array
    fn to_bytes(&self) -> Vec<u8> {
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
                use core::mem::{offset_of, size_of};
                
                FieldLayout {
                    fields: vec![
                        $(
                            impl_smbios_field_layout!(@field_info $struct_name, $field_name, $field_type),
                        )*
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
#[repr(C, packed)]
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
                "Default Vendor".to_string(),
                "1.0.0".to_string(),
                "01/01/2025".to_string(),
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
#[repr(C, packed)]
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
                "System Manufacturer".to_string(),
                "System Product".to_string(),
                "1.0".to_string(),
                "SystemSerial123".to_string(),
                "SystemSKU".to_string(),
                "System Family".to_string(),
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

/// Even Better: Derive Macro Approach
/// 
/// For maximum scalability, we could provide derive macros that generate 
/// everything automatically:

use smbios_derive::SmbiosRecord;

#[derive(SmbiosRecord)]
#[smbios(record_type = 2)]
pub struct Type2BaseboardInformation {
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
    
    #[smbios(string_pool)]
    pub strings: Vec<String>,
}

// The derive macro would automatically generate:
// - impl SmbiosFieldLayout 
// - impl SmbiosRecordStructure with proper RECORD_TYPE
// - Standard validation for strings
// - String pool accessors

/// Type 3: System Enclosure - another example showing how simple it becomes
#[derive(SmbiosRecord)] 
#[smbios(record_type = 3)]
pub struct Type3SystemEnclosure {
    pub manufacturer: u8,         // String index
    pub enclosure_type: u8,
    pub version: u8,              // String index
    pub serial_number: u8,        // String index
    pub asset_tag_number: u8,     // String index
    pub bootup_state: u8,
    pub power_supply_state: u8,
    pub thermal_state: u8,
    pub security_status: u8,
    pub oem_defined: u32,
    pub height: u8,
    pub number_of_power_cords: u8,
    pub contained_element_count: u8,
    pub contained_element_record_length: u8,
    
    #[smbios(string_pool)]
    pub strings: Vec<String>,
}

// That's it! No manual serialization code needed.

/// OEM Record becomes equally simple:
#[derive(SmbiosRecord)]
#[smbios(record_type = 0x80)]
pub struct OemCustomRecord {
    pub oem_field1: u32,
    pub oem_field2: u16,
    pub oem_string_ref: u8,       // String index
    #[smbios(array_len = 8)]
    pub reserved: [u8; 8],
    
    #[smbios(string_pool)]
    pub strings: Vec<String>,
}
```

### Enhanced Validation in Service Implementation

With this approach, the service focuses on validation rather than complex type management:

```rust
impl SmbiosRecords for SmbiosManager {
    fn add_from_bytes(
        &mut self,
        producer_handle: Option<Handle>,
        record_data: &[u8],
    ) -> Result<SmbiosHandle, SmbiosError> {
        let _lock = self.lock.lock().unwrap();

        // Enhanced validation as suggested in the comment
        
        // 1. Validate minimum size for header (at least 4 bytes)
        if record_data.len() < core::mem::size_of::<SmbiosTableHeader>() {
            return Err(SmbiosError::BufferTooSmall);
        }

        // 2. Parse and validate header
        let header = unsafe {
            &*(record_data.as_ptr() as *const SmbiosTableHeader)
        };
        
        // 3. Validate header->length is <= (record_data.length - 2) for string pool
        if (header.length as usize + 2) > record_data.len() {
            return Err(SmbiosError::BufferTooSmall);
        }
        
        // 4. Validate and count strings in a single efficient pass
        let string_pool_start = header.length as usize;
        let string_pool_area = &record_data[string_pool_start..];
        
        if string_pool_area.len() < 2 {
            return Err(SmbiosError::InvalidParameter);
        }
        
        // 5. Validate string pool format and count strings in one pass
        let string_count = Self::validate_and_count_strings(string_pool_area)?;
        
        // If all validation passes, proceed with record addition
        let smbios_handle = self.allocate_handle()?;
        
        let mut record_header = *header;
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
    /// ```
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
    /// ```
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
}
```

### Usage Examples

#### Creating a Type 0 Record: Complete Example

Here's a comprehensive example showing how a Type 0 (BIOS Information) record gets created step by step:

```rust
use patina_smbios::{SmbiosRecords, Type0PlatformFirmwareInformation, SmbiosRecordStructure};

/// Configuration structure for BIOS information
pub struct BiosConfig {
    pub vendor: String,
    pub firmware_version: String,
    pub release_date: String,
    pub major_release: u8,
    pub minor_release: u8,
    pub rom_size: u8,
    pub characteristics: u64,
}

/// Step-by-step Type 0 record creation
fn create_and_add_type0_record(
    smbios_records: &mut dyn SmbiosRecords,
    config: BiosConfig,
) -> Result<SmbiosHandle, SmbiosError> {
    
    // Step 1: Create the basic structure with defaults
    let mut bios_info = Type0PlatformFirmwareInformation::new();
    
    // Step 2: Customize the structure with your specific data
    bios_info = bios_info
        .with_vendor(config.vendor)?
        .with_firmware_version(config.firmware_version)?
        .with_release_date(config.release_date)?;
    
    // Step 3: Set additional fields directly
    bios_info.system_bios_major_release = config.major_release;
    bios_info.system_bios_minor_release = config.minor_release;
    bios_info.firmware_rom_size = config.rom_size;
    bios_info.characteristics = config.characteristics;
    
    // Step 4: Validate the structure
    bios_info.validate()?;
    
    // Step 5: Convert to bytes (this is where the magic happens)
    let record_bytes = bios_info.to_bytes();
    
    // Step 6: Add via the simple byte array interface
    let handle = smbios_records.add_from_bytes(None, &record_bytes)?;
    
    log::info!("Created Type 0 record with handle: {}", handle);
    log::debug!("Record size: {} bytes", record_bytes.len());
    
    Ok(handle)
}

/// Example usage in a component
#[derive(IntoComponent)]
struct BiosInfoComponent;

impl BiosInfoComponent {
    fn entry_point(
        self,
        smbios_records: Service<dyn SmbiosRecords>,
    ) -> patina_sdk::error::Result<()> {
        let bios_config = BiosConfig {
            vendor: "Patina Firmware Corp".to_string(),
            firmware_version: "v2.5.1-patina".to_string(),
            release_date: "09/18/2025".to_string(),
            major_release: 2,
            minor_release: 5,
            rom_size: 0x10, // 2MB (calculated as (size in KB / 64KB) - 1)
            characteristics: 0x08 | 0x10, // PCI supported + PnP supported
        };
        
        let handle = create_and_add_type0_record(&mut *smbios_records, bios_config)?;
        
        log::info!("Successfully installed BIOS Information record with handle: {}", handle);
        Ok(())
    }
}
```

#### What Happens Under the Hood: Data Flow

Let's trace through what happens when `to_bytes()` is called:

```rust
impl Type0PlatformFirmwareInformation {
    // This method creates the actual byte array that gets sent to add_from_bytes()
    fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        
        // Step 1: Calculate structured data size (everything except strings)
        let structured_size = 
            4 +    // SmbiosTableHeader (type, length, handle) 
            1 +    // vendor (string index)
            1 +    // firmware_version (string index) 
            2 +    // bios_starting_address_segment
            1 +    // firmware_release_date (string index)
            1 +    // firmware_rom_size
            8 +    // characteristics (u64)
            1 +    // characteristics_ext1
            1 +    // characteristics_ext2  
            1 +    // system_bios_major_release
            1 +    // system_bios_minor_release
            1 +    // embedded_controller_major_release
            1 +    // embedded_controller_minor_release
            2;     // extended_bios_rom_size
        // Total: 25 bytes of structured data
        
        // Step 2: Create header with correct length
        let mut header = self.header;
        header.length = structured_size as u8; // 25
        header.record_type = 0; // Type 0
        
        // Step 3: Add header bytes [0x00, 0x19, 0xFE, 0xFF]
        //   0x00 = Type 0
        //   0x19 = Length (25 decimal)  
        //   0xFE, 0xFF = Handle (SMBIOS_HANDLE_PI_RESERVED)
        bytes.extend_from_slice(&[
            header.record_type,
            header.length, 
            (header.handle & 0xFF) as u8,
            ((header.handle >> 8) & 0xFF) as u8,
        ]);
        
        // Step 4: Add structured data fields in SMBIOS spec order
        bytes.push(self.vendor);                    // 0x01 (first string)
        bytes.push(self.firmware_version);          // 0x02 (second string) 
        bytes.extend_from_slice(&self.bios_starting_address_segment.to_le_bytes()); // 0x00, 0xE0
        bytes.push(self.firmware_release_date);     // 0x03 (third string)
        bytes.push(self.firmware_rom_size);         // 0x10
        bytes.extend_from_slice(&self.characteristics.to_le_bytes()); // 8 bytes
        bytes.push(self.characteristics_ext1);      // 0x01
        bytes.push(self.characteristics_ext2);      // 0x00
        bytes.push(self.system_bios_major_release); // 0x02
        bytes.push(self.system_bios_minor_release); // 0x05
        bytes.push(self.embedded_controller_major_release); // 0xFF
        bytes.push(self.embedded_controller_minor_release); // 0xFF
        bytes.extend_from_slice(&self.extended_bios_rom_size.to_le_bytes()); // 0x00, 0x00
        
        // Step 5: Add string pool
        // Strings: ["Patina Firmware Corp", "v2.5.1-patina", "09/18/2025"]
        for string in &self.string_pool {
            if !string.is_empty() {
                bytes.extend_from_slice(string.as_bytes());
            }
            bytes.push(0); // Null terminator for each string
        }
        bytes.push(0); // Double null terminator
        
        bytes
    }
}
```

#### Final Byte Array Structure

For the example above, the final byte array would look like this:

```text
Offset | Bytes                           | Description
-------|--------------------------------|------------------------
0x00   | 00 19 FE FF                    | Header: Type=0, Len=25, Handle=0xFFFE
0x04   | 01                             | Vendor string index (1st string)
0x05   | 02                             | Version string index (2nd string)  
0x06   | 00 E0                          | BIOS segment (0xE000)
0x08   | 03                             | Release date string index (3rd string)
0x09   | 10                             | ROM size (2MB)
0x0A   | 18 00 00 00 00 00 00 00        | Characteristics (0x18 = PCI+PnP)
0x12   | 01                             | Extension 1 (ACPI supported)
0x13   | 00                             | Extension 2
0x14   | 02                             | BIOS Major (2)
0x15   | 05                             | BIOS Minor (5)  
0x16   | FF                             | EC Major (not supported)
0x17   | FF                             | EC Minor (not supported)
0x18   | 00 00                          | Extended ROM size
-------|--------------------------------|------------------------
0x1A   | 50 61 74 69 6E 61 20 46 69... | "Patina Firmware Corp\0"
       | 76 32 2E 35 2E 31 2D 70 61... | "v2.5.1-patina\0"  
       | 30 39 2F 31 38 2F 32 30 32... | "09/18/2025\0"
       | 00                             | Double null terminator
```

#### Validation During add_from_bytes()

When this byte array is passed to `add_from_bytes()`, it performs the validation requested in the comment:

```rust
impl SmbiosManager {
    fn add_from_bytes(&mut self, producer_handle: Option<Handle>, record_data: &[u8]) 
        -> Result<SmbiosHandle, SmbiosError> {
        
        // 1. ✅ Check record_data is at least 4 bytes for header
        if record_data.len() < 4 {
            return Err(SmbiosError::BufferTooSmall);
        }
        
        // 2. ✅ Parse header and validate length field
        let header_length = record_data[1] as usize; // 25 in our example
        if (header_length + 2) > record_data.len() {
            return Err(SmbiosError::BufferTooSmall); 
        }
        
        // 3. ✅ Check string pool area (from offset 25 to end)
        let string_area = &record_data[header_length..];
        
        // 4. ✅ Validate only single nulls in string pool
        let mut consecutive_nulls = 0;
        for &byte in &string_area[..string_area.len()-1] {
            if byte == 0 {
                consecutive_nulls += 1;
                if consecutive_nulls > 1 {
                    return Err(SmbiosError::InvalidParameter);
                }
            } else {
                consecutive_nulls = 0;
            }
        }
        
        // 5. ✅ Validate double NULL at end
        if !record_data.ends_with(&[0, 0]) {
            return Err(SmbiosError::InvalidParameter);
        }
        
        // All validation passed - proceed with adding the record
        // ...
    }
}
```

This approach gives you:

- **Type safety** during construction (the struct prevents many errors)
- **Flexibility** for OEMs to define custom structures  
- **Comprehensive validation** at the service boundary
- **Simple interface** (single `add_from_bytes` method)

#### Adding a System Information Record

```rust
fn add_system_information(
    smbios_records: &mut dyn SmbiosRecords,
    config: &SystemConfig,
) -> Result<SmbiosHandle, SmbiosError> {
    let mut system_info = Type1SystemInformation::new();
    
    // Update string pool with actual system information
    system_info.string_pool = vec![
        config.manufacturer.clone(),
        config.product_name.clone(), 
        config.version.clone(),
        config.serial_number.clone(),
        config.sku_number.clone(),
        config.family.clone(),
    ];
    
    // Set UUID from configuration
    system_info.uuid = config.system_uuid;
    
    system_info.validate()?;
    let record_bytes = system_info.to_bytes();
    smbios_records.add_from_bytes(None, &record_bytes)
}
```

#### OEM Custom Record (Type 0x80-0xFE)

```rust
/// Example OEM-specific record structure
#[repr(C, packed)]  
pub struct OemCustomRecord {
    pub header: SmbiosTableHeader,
    pub oem_field1: u32,
    pub oem_field2: u16, 
    pub oem_string_ref: u8,     // String index
    pub reserved: [u8; 8],
    
    pub string_pool: Vec<String>,
}

impl SmbiosRecordStructure for OemCustomRecord {
    const RECORD_TYPE: u8 = 0x80; // OEM-specific type
    
    fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        
        let structured_size = core::mem::size_of::<SmbiosTableHeader>() +
                             core::mem::size_of::<u32>() +     // oem_field1
                             core::mem::size_of::<u16>() +     // oem_field2
                             core::mem::size_of::<u8>() +      // oem_string_ref
                             8; // reserved
        
        let mut header = self.header;
        header.length = structured_size as u8;
        header.record_type = Self::RECORD_TYPE;
        
        // Serialize header
        let header_bytes = unsafe {
            core::slice::from_raw_parts(
                &header as *const _ as *const u8,
                core::mem::size_of::<SmbiosTableHeader>(),
            )
        };
        bytes.extend_from_slice(header_bytes);
        
        // Serialize fields
        bytes.extend_from_slice(&self.oem_field1.to_le_bytes());
        bytes.extend_from_slice(&self.oem_field2.to_le_bytes());
        bytes.push(self.oem_string_ref);
        bytes.extend_from_slice(&self.reserved);
        
        // Add string pool
        if self.string_pool.is_empty() {
            bytes.extend_from_slice(&[0, 0]);
        } else {
            for string in &self.string_pool {
                if !string.is_empty() {
                    bytes.extend_from_slice(string.as_bytes());
                }
                bytes.push(0);
            }
            bytes.push(0);
        }
        
        bytes
    }
    
    fn validate(&self) -> Result<(), SmbiosError> {
        // OEM-specific validation
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
}

impl OemCustomRecord {
    pub fn new(oem_data: u32, description: String) -> Result<Self, SmbiosError> {
        if description.len() > SMBIOS_STRING_MAX_LENGTH {
            return Err(SmbiosError::StringTooLong);
        }
        
        Ok(Self {
            header: SmbiosTableHeader {
                record_type: Self::RECORD_TYPE,
                length: 0, // Will be calculated
                handle: SMBIOS_HANDLE_PI_RESERVED,
            },
            oem_field1: oem_data,
            oem_field2: 0x1234, // OEM-specific value
            oem_string_ref: 1,   // First string
            reserved: [0; 8],
            string_pool: vec![description],
        })
    }
}

// Usage:
fn add_oem_record(
    smbios_records: &mut dyn SmbiosRecords
) -> Result<SmbiosHandle, SmbiosError> {
    let oem_record = OemCustomRecord::new(
        0xDEADBEEF, 
        "OEM Custom Data Record".to_string()
    )?;
    
    oem_record.validate()?;
    let bytes = oem_record.to_bytes();
    smbios_records.add_from_bytes(None, &bytes)
}
```

### Scalability Benefits of Generic Serialization

#### Code Reduction Comparison

| Approach | Lines of Code per Record Type | Maintenance Burden |
|----------|------------------------------|-------------------|
| **Custom to_bytes()** | ~80-120 lines | High - repetitive, error-prone |
| **Generic + Macro** | ~15-25 lines | Low - declarative field layout |
| **Derive Macro** | ~8-12 lines | Minimal - just struct definition |

#### Performance Characteristics

```rust
// The generic serializer is zero-cost at runtime:
// - Field layouts are computed at compile time
// - No dynamic dispatch or reflection
// - Unsafe code is isolated and well-tested
// - Memory layout matches exactly what manual code would produce

#[cfg(test)]
mod benchmarks {
    use super::*;
    
    #[bench]
    fn bench_generic_serialization(b: &mut Bencher) {
        let bios_info = Type0PlatformFirmwareInformation::new();
        
        b.iter(|| {
            // This compiles to the same assembly as hand-written serialization
            black_box(bios_info.to_bytes())
        });
    }
    
    #[bench] 
    fn bench_validation(b: &mut Bencher) {
        let record_bytes = create_test_type0_bytes();
        
        b.iter(|| {
            // Validation is O(n) where n is record size, not number of types
            black_box(validate_smbios_record(&record_bytes))
        });
    }
}
```

#### Supporting 50+ Record Types

With this approach, adding all standard SMBIOS types becomes manageable:

```rust
// Standard SMBIOS types become trivial to add:

#[derive(SmbiosRecord)]
#[smbios(record_type = 4)]
pub struct Type4ProcessorInformation {
    pub socket_designation: u8,      // String index
    pub processor_type: u8,
    pub processor_family: u8,
    pub processor_manufacturer: u8,  // String index  
    pub processor_id: u64,
    pub processor_version: u8,       // String index
    pub voltage: u8,
    pub external_clock: u16,
    pub max_speed: u16,
    pub current_speed: u16,
    pub status: u8,
    pub processor_upgrade: u8,
    pub l1_cache_handle: u16,
    pub l2_cache_handle: u16,
    pub l3_cache_handle: u16,
    pub serial_number: u8,           // String index
    pub asset_tag: u8,              // String index
    pub part_number: u8,            // String index
    pub core_count: u8,
    pub core_enabled: u8,
    pub thread_count: u8,
    pub processor_characteristics: u16,
    pub processor_family_2: u16,
    pub core_count_2: u16,
    pub core_enabled_2: u16,
    pub thread_count_2: u16,
    
    #[smbios(string_pool)]
    pub strings: Vec<String>,
}

// Memory Device (Type 17) - one of the most complex types
#[derive(SmbiosRecord)]
#[smbios(record_type = 17)]
pub struct Type17MemoryDevice {
    pub physical_memory_array_handle: u16,
    pub memory_error_information_handle: u16,
    pub total_width: u16,
    pub data_width: u16,
    pub size: u16,
    pub form_factor: u8,
    pub device_set: u8,
    pub device_locator: u8,          // String index
    pub bank_locator: u8,            // String index
    pub memory_type: u8,
    pub type_detail: u16,
    pub speed: u16,
    pub manufacturer: u8,            // String index
    pub serial_number: u8,           // String index
    pub asset_tag: u8,              // String index
    pub part_number: u8,            // String index
    pub attributes: u8,
    pub extended_size: u32,
    pub configured_memory_speed: u16,
    pub minimum_voltage: u16,
    pub maximum_voltage: u16,
    pub configured_voltage: u16,
    pub memory_technology: u8,
    pub memory_operating_mode_capability: u16,
    pub firmware_version: u8,        // String index
    pub module_manufacturer_id: u16,
    pub module_product_id: u16,
    pub memory_subsystem_controller_manufacturer_id: u16,
    pub memory_subsystem_controller_product_id: u16,
    pub non_volatile_size: u64,
    pub volatile_size: u64,
    pub cache_size: u64,
    pub logical_size: u64,
    
    #[smbios(string_pool)]
    pub strings: Vec<String>,
}

// Even the most complex types require minimal code!
```

#### Factory Pattern for Dynamic Type Support

```rust
/// Registry of all supported SMBIOS record types
pub struct SmbiosTypeRegistry;

impl SmbiosTypeRegistry {
    /// Create a record structure from type ID (useful for dynamic scenarios)
    pub fn create_default_record(record_type: u8) -> Result<Box<dyn SmbiosRecordStructure>, SmbiosError> {
        match record_type {
            0 => Ok(Box::new(Type0PlatformFirmwareInformation::new())),
            1 => Ok(Box::new(Type1SystemInformation::new())),
            2 => Ok(Box::new(Type2BaseboardInformation::new())),
            3 => Ok(Box::new(Type3SystemEnclosure::new())),
            4 => Ok(Box::new(Type4ProcessorInformation::new())),
            // ... all 50+ types
            17 => Ok(Box::new(Type17MemoryDevice::new())),
            0x80..=0xFE => Err(SmbiosError::UnsupportedRecordType), // OEM-specific, must be created explicitly
            _ => Err(SmbiosError::UnsupportedRecordType),
        }
    }
    
    /// Validate any record type generically
    pub fn validate_record_bytes(record_bytes: &[u8]) -> Result<(), SmbiosError> {
        if record_bytes.len() < 4 {
            return Err(SmbiosError::BufferTooSmall);
        }
        
        let record_type = record_bytes[0];
        let header_length = record_bytes[1] as usize;
        
        // Generic validation works for all types!
        SmbiosSerializer::validate_generic_record(record_bytes, record_type, header_length)
    }
}
```

### Benefits of This Scalable Alternative Approach

1. **Massive Code Reduction**: 80+ lines per type → 8-12 lines per type
2. **Consistency**: All types serialized identically, no per-type bugs
3. **Maintainability**: Adding new SMBIOS spec versions requires minimal changes
4. **Performance**: Zero-cost abstractions, identical assembly to manual code
5. **Type Safety**: Compile-time field layout verification
6. **OEM Extensibility**: Same simple pattern works for custom record types
7. **Testing**: Generic serializer can be thoroughly tested once vs. 50+ custom implementations

### Comparison with Versioned Typed Interfaces

| Aspect | Sized Structures + Byte Arrays | Versioned Typed Interfaces |
|--------|--------------------------------|----------------------------|
| **API Complexity** | Simple, single method | More complex trait system |
| **Version Handling** | Manual, per-structure | Automatic, built-in |
| **OEM Extensibility** | Excellent, clear pattern | Good, but more complex |
| **Validation** | Explicit, comprehensive | Implicit in parsing |
| **Forward Compatibility** | Manual handling required | Automatic preservation |
| **Learning Curve** | Gentle, familiar patterns | Steeper, more abstractions |

Both approaches have merit, and the choice depends on whether the implementation prioritizes simplicity
and explicit control (sized structures + byte arrays) or automatic version handling and forward compatibility
(versioned typed interfaces).

## Unresolved Questions

No remaining unresolved questions.

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
is provided through the `SmbiosRecords` service.

```rust
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
    /// 
    /// Returns a boxed iterator to maintain trait object safety for service registration.
    fn iter(&self) -> Box<dyn Iterator<Item = &'a SmbiosRecord> + 'a>;

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
    next_handle: SmbiosHandle,
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
    
    /// Check if a handle is already in use by scanning the records Vec.
    /// 
    /// This approach eliminates the need for a separate HashSet, reducing memory
    /// overhead and implementation complexity. Since SMBIOS typically has a small
    /// number of records (usually < 100), the O(n) scan is acceptable.
    fn is_handle_allocated(&self, handle: SmbiosHandle) -> bool {
        self.records.iter().any(|r| r.header.handle == handle)
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
    
    /// # Safety
    /// 
    /// This method is unsafe because it assumes that `header` points to a complete
    /// SMBIOS record structure where the header is followed by the structured data.
    /// The caller must ensure that `header` points to a valid, complete SMBIOS record
    /// with at least `header.length` bytes of valid memory.
    unsafe fn build_record_with_strings(
        header: &SmbiosTableHeader,
        strings: &[&str],
    ) -> Result<Vec<u8>, SmbiosError> {
        // Validate all strings first
        for s in strings {
            Self::validate_string(s)?;
        }
        
        let mut record = Vec::new();
        
        // Add the structured data
        // SAFETY: Caller guarantees that header points to a complete record structure
        let header_bytes = std::slice::from_raw_parts(
            header as *const _ as *const u8,
            header.length as usize,
        );
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

    /// Allocates a unique SMBIOS handle using a pure counter-based approach.
    ///
    /// Performance characteristics:
    /// - O(1) average case when handles are allocated sequentially
    /// - O(n) average case when handles are sparse, where n = number of records
    /// - O(n²) worst case when handle space is nearly full
    /// 
    /// The algorithm uses a simple counter (`next_handle`) that tracks the likely
    /// next available handle. When a collision occurs, it searches the records Vec
    /// to find the next available handle, eliminating the need for a separate HashSet.
    fn allocate_handle(&mut self) -> Result<SmbiosHandle, SmbiosError> {
        let start_handle = self.next_handle;
        
        // Try sequential allocation first (O(1) in common case)
        for offset in 0..0xFEFF {
            // Calculate handle with wraparound: 1-based, wraps from 0xFEFF to 1
            let handle = if start_handle + offset >= 0xFF00 {
                (start_handle + offset) - 0xFEFF + 1
            } else {
                start_handle + offset
            };
            
            // Check if handle is already in use
            if !self.is_handle_allocated(handle) {
                // Update next_handle for next allocation
                self.next_handle = if handle >= 0xFEFF { 1 } else { handle + 1 };
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

    unsafe fn add(
        &mut self,
        producer_handle: Option<Handle>,
        record: &SmbiosTableHeader,
    ) -> Result<SmbiosHandle, SmbiosError> {
        let _lock = self.lock.lock();

        // Assign handle 
        let smbios_handle = self.allocate_handle()?;

        // This is unsafe because we're assuming that `record` points to a complete
        // SMBIOS record structure where the header is followed by the record data.
        // The caller must ensure that `record` points to a valid, complete SMBIOS record
        // with at least `record.length` bytes of valid memory.
        let data = unsafe {
            // Create record data (simplified - would need proper string parsing)
            let record_size = record.length as usize;
            let mut data = Vec::with_capacity(record_size + 2); // +2 for double null
            
            let bytes = std::slice::from_raw_parts(
                record as *const _ as *const u8,
                record_size,
            );

            data.extend_from_slice(bytes);
            
            // Add double null terminator (simplified)
            data.extend_from_slice(&[0, 0]);
            data
        };



        let mut record_header = *record;
        record_header.handle = smbios_handle;

        // Calculate string count from the data
        let string_count = Self::count_strings_in_record(&data)?;

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

    fn add_from_bytes(
        &mut self,
        producer_handle: Option<Handle>,
        record_data: &[u8],
    ) -> Result<SmbiosHandle, SmbiosError> {
        let _lock = self.lock.lock().unwrap();

        // Validate minimum size for header
        if record_data.len() < core::mem::size_of::<SmbiosTableHeader>() {
            return Err(SmbiosError::InvalidHandle);
        }

        // Parse header from the byte data
        let header = unsafe {
            &*(record_data.as_ptr() as *const SmbiosTableHeader)
        };

        // Validate that the record data is at least as long as the header claims
        if record_data.len() < header.length as usize {
            return Err(SmbiosError::InvalidHandle);
        }

        // Assign handle 
        let smbios_handle = self.allocate_handle()?;
        
        // Create a safe copy of the data with updated handle
        let mut data = Vec::with_capacity(record_data.len() + 2); // +2 for double null
        data.extend_from_slice(record_data);
        
        // Add double null terminator if not already present (simplified)
        if !data.ends_with(&[0, 0]) {
            data.extend_from_slice(&[0, 0]);
        }

        let mut record_header = *header;
        record_header.handle = smbios_handle;

        // Calculate string count from the data
        let string_count = Self::count_strings_in_record(&data)?;

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
        let _lock = self.lock.lock();
        
        let pos = self.records
            .iter()
            .position(|r| r.header.handle == smbios_handle)
            .ok_or(SmbiosError::NotFound)?;

        self.records.remove(pos);
        
        // Optimization: if we removed a handle lower than next_handle, 
        // we can potentially reuse it sooner by updating next_handle
        if smbios_handle < self.next_handle {
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

    fn iter(&self) -> Box<dyn Iterator<Item = &'static SmbiosRecord> + 'static> {
        // Return boxed iterator for trait object safety
        // This extends the lifetime using unsafe pointer arithmetic
        // Safe because SmbiosManager is 'static and records are only modified via &mut
        let records_ptr = self.records.as_ptr();
        let len = self.records.len();
        Box::new((0..len).map(move |i| unsafe { &*records_ptr.add(i) }))
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
        match unsafe { manager.lock().unwrap().add(Some(producer_handle), &header) } {
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
    let handle = unsafe { smbios_records.add(None, &record)? };
    
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
- Memory management for 64-bit SMBIOS 3.0+ tables
- Installation of system configuration tables

### BIOS Information Example (Type 0)

Below is an example of creating and installing a BIOS Information (Type 0) SMBIOS record using both the
traditional byte-based approach and the new versioned typed record interface:

#### Traditional Byte-Based Approach

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

        let record_data = unsafe {
            SmbiosManager::build_record_with_strings(
                &bios_info.header,
                &strings,
            )?
        };

        // Use the safe add_from_bytes method
        let handle = smbios_records.add_from_bytes(None, &record_data)?;

        log::info!("Added BIOS Information record with handle: {}", handle);
        Ok(())
    }
}
```

#### Versioned Typed Record Approach (Recommended)

```rust
#[derive(IntoComponent)]
struct SmbiosVersionedBiosInfoManager;

impl SmbiosVersionedBiosInfoManager {
    fn entry_point(
        self,
        config: Config<BiosInfoConfig>,
        smbios_records: Service<dyn SmbiosRecords>,
    ) -> patina_sdk::error::Result<()> {
        // Create a version-appropriate BIOS Information record
        let smbios_version = smbios_records.version();
        let mut bios_info = BiosInformation::new_for_version(smbios_version);
        
        // Set configuration values based on spec version capabilities
        if smbios_version >= (2, 4) {
            bios_info.major_release = Some(config.major_release);
            bios_info.minor_release = Some(config.minor_release);
        }
        
        // Use the typed record interface
        let handle = smbios_records.add_typed_record(None, &bios_info)?;

        log::info!(
            "Added BIOS Information record (SMBIOS {}.{}) with handle: {}", 
            smbios_version.0, 
            smbios_version.1,
            handle
        );
        
        // Demonstrate reading back the typed record
        let retrieved_bios_info: BiosInformation = 
            smbios_records.get_typed_record(handle)?;
        
        log::info!(
            "Retrieved BIOS info - parsed version: {}.{}, has extended fields: {}",
            retrieved_bios_info.parsed_version().0,
            retrieved_bios_info.parsed_version().1,
            retrieved_bios_info.major_release.is_some()
        );

        Ok(())
    }
}

/// Example of handling version compatibility scenarios
impl SmbiosVersionedBiosInfoManager {
    /// Demonstrate forward compatibility: newer record format on older implementation
    fn handle_forward_compatibility_example(
        &self,
        smbios_records: &dyn SmbiosRecords,
    ) -> Result<(), SmbiosError> {
        // Simulate receiving a SMBIOS 3.2 Type 0 record with unknown fields
        let future_bios_data = vec![
            // Standard SMBIOS 3.0 fields
            0x00, 0x20, 0x01, 0x00,  // Header: Type 0, Length 32, Handle 1
            0x01, 0x02, 0x00, 0xE0,  // Vendor, Version, Segment
            0x03, 0x0F,              // Date, Size
            0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // Characteristics
            0x01, 0x00,              // Extension bytes
            0x01, 0x00,              // Major/Minor release
            0xFF, 0xFF,              // EC releases
            0x00, 0x00,              // Extended BIOS size
            // Future fields our implementation doesn't recognize
            0xAB, 0xCD, 0xEF, 0x12,
        ];
        
        // Our implementation can safely parse this
        let current_version = smbios_records.version();
        let bios_info = BiosInformation::from_bytes(&future_bios_data, current_version)?;
        
        // Known fields are accessible
        assert_eq!(bios_info.vendor, 1);
        assert_eq!(bios_info.major_release, Some(1));
        
        // Unknown data is preserved
        assert_eq!(bios_info.unknown_data, vec![0xAB, 0xCD, 0xEF, 0x12]);
        
        // When we convert back to bytes, future fields are preserved
        let preserved_bytes = bios_info.to_bytes();
        let handle = smbios_records.add_from_bytes(None, &preserved_bytes)?;
        
        log::info!("Successfully handled future SMBIOS record format with handle: {}", handle);
        Ok(())
    }
    
    /// Demonstrate backward compatibility: older record on newer implementation  
    fn handle_backward_compatibility_example(
        &self,
        smbios_records: &dyn SmbiosRecords,
    ) -> Result<(), SmbiosError> {
        // Simulate an old SMBIOS 2.0 Type 0 record (minimal fields)
        let legacy_bios_data = vec![
            0x00, 0x12, 0x01, 0x00,  // Header: Type 0, Length 18, Handle 1
            0x01, 0x02, 0x00, 0xE0,  // Vendor, Version, Segment
            0x03, 0x0F,              // Date, Size
            0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // Characteristics
            // No extended fields (pre-SMBIOS 2.4)
        ];
        
        // Parse as SMBIOS 2.0 format
        let bios_info = BiosInformation::from_bytes(&legacy_bios_data, (2, 0))?;
        
        // Extended fields should be None for older format
        assert_eq!(bios_info.major_release, None);
        assert_eq!(bios_info.characteristics_ext1, None);
        
        // But we can still work with the record
        let handle = smbios_records.add_typed_record(None, &bios_info)?;
        
        log::info!("Successfully handled legacy SMBIOS 2.0 record with handle: {}", handle);
        Ok(())
    }
}
```

#### Iteration Over Typed Records

```rust
/// Example of iterating over typed BIOS Information records
fn enumerate_bios_info_records(
    smbios_records: &dyn SmbiosRecords
) -> Result<(), SmbiosError> {
    log::info!("Enumerating all BIOS Information records:");
    
    for (index, result) in smbios_records.iter_typed_records::<BiosInformation>().enumerate() {
        match result {
            Ok(bios_info) => {
                log::info!(
                    "  BIOS Info #{}: Parsed as SMBIOS {}.{}, Vendor string index: {}",
                    index,
                    bios_info.parsed_version().0,
                    bios_info.parsed_version().1,
                    bios_info.vendor
                );
                
                if let Some(major) = bios_info.major_release {
                    log::info!("    BIOS Release: {}.{}", 
                              major, 
                              bios_info.minor_release.unwrap_or(0));
                }
                
                if !bios_info.unknown_data.is_empty() {
                    log::info!("    Contains {} bytes of future/unknown data", 
                              bios_info.unknown_data.len());
                }
            }
            Err(e) => {
                log::error!("  Failed to parse BIOS Info record #{}: {:?}", index, e);
            }
        }
    }
    
    Ok(())
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

## String Count Calculation Examples

Here are practical examples showing how `string_count` is calculated for different SMBIOS record types:

### Example 1: Type 0 (BIOS Information) with 3 strings

```rust
// Create a Type 0 record with vendor, version, and release date
let bios_record = Type0PlatformFirmwareInformation::new()
    .with_vendor("Patina Firmware Corp".to_string())?
    .with_firmware_version("v2.5.1-patina".to_string())?
    .with_release_date("09/18/2025".to_string())?;

let record_bytes = bios_record.to_bytes();

// Byte layout:
// [0x00, 0x19, 0xFE, 0xFF] - Header (Type=0, Length=25, Handle=0xFFFE)
// [0x01] - Vendor string index (1st string)
// [0x02] - Version string index (2nd string)  
// [0x00, 0xE0] - BIOS segment
// [0x03] - Release date string index (3rd string)
// ... (additional structured fields)
// ["Patina Firmware Corp\0"] - String 1 (21 bytes + null)
// ["v2.5.1-patina\0"] - String 2 (14 bytes + null) 
// ["09/18/2025\0"] - String 3 (10 bytes + null)
// [0x00] - Double null terminator

// String counting process:
let string_count = SmbiosManager::count_strings_in_record(&record_bytes)?;
assert_eq!(string_count, 3); // Found 3 non-empty strings
```

### Example 2: Type 1 (System Information) with 6 strings

```rust
let mut system_info = Type1SystemInformation::new();
system_info.string_pool = vec![
    "ACME Corp".to_string(),           // String 1 - Manufacturer
    "SuperServer X123".to_string(),    // String 2 - Product Name
    "Rev 2.1".to_string(),            // String 3 - Version
    "SN123456789".to_string(),        // String 4 - Serial Number
    "SKU-X123-BASE".to_string(),      // String 5 - SKU Number
    "Server Family".to_string(),       // String 6 - Family
];

let record_bytes = system_info.to_bytes();

// The string pool section would be:
// ["ACME Corp\0SuperServer X123\0Rev 2.1\0SN123456789\0SKU-X123-BASE\0Server Family\0\0"]
//      ^1         ^2              ^3        ^4           ^5             ^6           ^^
//                                                                                   ||
//                                                                        String end ||
//                                                                     Double null --+

let string_count = SmbiosManager::count_strings_in_record(&record_bytes)?;
assert_eq!(string_count, 6); // Found 6 non-empty strings
```

### Example 3: Record with no strings (empty string pool)

```rust
// A record type that doesn't use strings (e.g., some OEM records)
let oem_record = OemCustomRecord {
    header: SmbiosTableHeader { 
        record_type: 0x80, 
        length: 16, 
        handle: SMBIOS_HANDLE_PI_RESERVED 
    },
    oem_field1: 0x12345678,
    oem_field2: 0xABCD,
    reserved: [0; 8],
    string_pool: Vec::new(), // No strings
};

let record_bytes = oem_record.to_bytes();

// Byte layout:
// [0x80, 0x10, 0xFE, 0xFF] - Header
// [0x78, 0x56, 0x34, 0x12] - OEM field 1 (little-endian)
// [0xCD, 0xAB] - OEM field 2 (little-endian)  
// [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00] - Reserved
// [0x00, 0x00] - Empty string pool (just double null)

let string_count = SmbiosManager::count_strings_in_record(&record_bytes)?;
assert_eq!(string_count, 0); // No strings found
```

### Example 4: Step-by-step string counting algorithm

```rust
fn demonstrate_string_counting(record_data: &[u8]) -> Result<usize, SmbiosError> {
    println!("=== String Counting Demonstration ===");
    
    // Step 1: Extract header length
    let header_length = record_data[1] as usize;
    println!("Header length: {} bytes", header_length);
    
    // Step 2: Find string pool start
    let string_pool_start = header_length;
    println!("String pool starts at offset: {}", string_pool_start);
    
    // Step 3: Extract string pool
    let string_pool = &record_data[string_pool_start..];
    println!("String pool size: {} bytes", string_pool.len());
    
    // Step 4: Validate double null terminator
    let len = string_pool.len();
    if len < 2 || string_pool[len - 1] != 0 || string_pool[len - 2] != 0 {
        return Err(SmbiosError::InvalidParameter);
    }
    println!("✓ Valid double null terminator found");
    
    // Step 5: Count strings
    let mut count = 0;
    let mut i = 0;
    let data_end = len - 2; // Exclude final double-null
    
    while i < data_end {
        let start = i;
        
        // Find null terminator
        while i < data_end && string_pool[i] != 0 {
            i += 1;
        }
        
        if i > start {
            let string_bytes = &string_pool[start..i];
            let string_text = String::from_utf8_lossy(string_bytes);
            println!("String {}: \"{}\" ({} bytes)", count + 1, string_text, i - start);
            count += 1;
        }
        
        i += 1; // Skip null terminator
    }
    
    println!("Total strings found: {}", count);
    Ok(count)
}

// Example usage:
let bios_bytes = create_bios_record_with_strings();
let count = demonstrate_string_counting(&bios_bytes)?;
// Output:
// === String Counting Demonstration ===
// Header length: 25 bytes
// String pool starts at offset: 25
// String pool size: 48 bytes
// ✓ Valid double null terminator found
// String 1: "Patina Firmware Corp" (20 bytes)
// String 2: "v2.5.1-patina" (14 bytes)  
// String 3: "09/18/2025" (10 bytes)
// Total strings found: 3
```

### Error Cases in String Counting

```rust
// Case 1: Invalid - single null (missing double null)
let invalid_single_null = vec![
    0x00, 0x04, 0xFE, 0xFF,  // Header
    0x00                      // Single null (invalid)
];
let result = SmbiosManager::count_strings_in_record(&invalid_single_null);
assert_eq!(result, Err(SmbiosError::InvalidParameter));

// Case 2: Invalid - string too long
let invalid_long_string = {
    let mut data = vec![0x00, 0x04, 0xFE, 0xFF]; // Header
    data.extend_from_slice(&vec![b'A'; SMBIOS_STRING_MAX_LENGTH + 1]); // Too long
    data.extend_from_slice(&[0x00, 0x00]); // Double null
    data
};
let result = SmbiosManager::count_strings_in_record(&invalid_long_string);
assert_eq!(result, Err(SmbiosError::StringTooLong));

// Case 3: Invalid - consecutive nulls in middle
let invalid_consecutive_nulls = vec![
    0x00, 0x04, 0xFE, 0xFF,           // Header
    b'H', b'i', 0x00,                // "Hi"
    0x00,                             // Second null (creates consecutive nulls)
    b'B', b'y', b'e', 0x00,          // "Bye"
    0x00                              // Final null
];
let result = SmbiosManager::count_strings_in_record(&invalid_consecutive_nulls);
assert_eq!(result, Err(SmbiosError::InvalidParameter));
```

### String Counting Performance

The string counting algorithm is designed for efficiency:

- **Single Pass**: Validation and counting happen in one pass through the data
- **Early Validation**: Header and format validation before string processing
- **Minimal Allocations**: No string copies or temporary allocations during counting  
- **Bounds Checking**: All array accesses are bounds-checked
- **O(n) Complexity**: Linear time complexity where n = string pool size

For typical SMBIOS records with 0-10 strings totaling <500 bytes, string counting takes microseconds.

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
        let record_data = unsafe {
            SmbiosManager::build_record_with_strings(
                &bios_info.header, 
                &strings
            ).unwrap()
        };
        
        let result = manager.add_from_bytes(None, &record_data);
        
        assert!(result.is_ok());
        let handle = result.unwrap();
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
        let record_data = unsafe {
            SmbiosManager::build_record_with_strings(
                &bios_info.header, 
                &strings
            ).unwrap()
        };
        
        let result1 = manager.add_from_bytes(None, &record_data);
        assert!(result1.is_ok());
        
        // Second add with same data should succeed and get different handle
        let result2 = manager.add_from_bytes(None, &record_data);
        assert!(result2.is_ok());
        assert_ne!(result1.unwrap(), result2.unwrap());
    }

    #[test]
    fn test_count_strings_in_record_basic() {
        // Test with 3 strings: "Vendor", "Version", "Date"
        let record_data = vec![
            0x00, 0x04, 0xFE, 0xFF,           // Header: Type=0, Length=4, Handle=0xFFFE
            b'V', b'e', b'n', b'd', b'o', b'r', 0x00,  // "Vendor\0"
            b'V', b'e', b'r', b's', b'i', b'o', b'n', 0x00, // "Version\0" 
            b'D', b'a', b't', b'e', 0x00,     // "Date\0"
            0x00                              // Double null terminator
        ];
        
        let count = SmbiosManager::count_strings_in_record(&record_data).unwrap();
        assert_eq!(count, 3);
    }

    #[test]
    fn test_count_strings_in_record_empty() {
        // Test with no strings (empty string pool)
        let record_data = vec![
            0x00, 0x04, 0xFE, 0xFF,  // Header: Type=0, Length=4, Handle=0xFFFE
            0x00, 0x00               // Empty string pool (double null)
        ];
        
        let count = SmbiosManager::count_strings_in_record(&record_data).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_count_strings_in_record_single_string() {
        // Test with single string: "OnlyOne"
        let record_data = vec![
            0x01, 0x08, 0x01, 0x00,           // Header: Type=1, Length=8, Handle=0x0001
            0x01, 0x02, 0x03, 0x04,           // Additional structured data
            b'O', b'n', b'l', b'y', b'O', b'n', b'e', 0x00, // "OnlyOne\0"
            0x00                              // Double null terminator
        ];
        
        let count = SmbiosManager::count_strings_in_record(&record_data).unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_count_strings_validation_errors() {
        // Test buffer too small
        let tiny_buffer = vec![0x00, 0x04]; // Only 2 bytes, need at least 4
        let result = SmbiosManager::count_strings_in_record(&tiny_buffer);
        assert_eq!(result, Err(SmbiosError::BufferTooSmall));
        
        // Test header length exceeds buffer
        let invalid_header = vec![
            0x00, 0xFF, 0xFE, 0xFF,  // Header claims length=255 but buffer is only 6 bytes
            0x00, 0x00
        ];
        let result = SmbiosManager::count_strings_in_record(&invalid_header);
        assert_eq!(result, Err(SmbiosError::BufferTooSmall));
        
        // Test missing double null terminator (single null)
        let single_null = vec![
            0x00, 0x04, 0xFE, 0xFF,  // Header
            0x00                      // Single null instead of double
        ];
        let result = SmbiosManager::count_strings_in_record(&single_null);
        assert_eq!(result, Err(SmbiosError::InvalidParameter));
        
        // Test string too long
        let mut long_string_data = vec![0x00, 0x04, 0xFE, 0xFF]; // Header
        long_string_data.extend_from_slice(&vec![b'A'; SMBIOS_STRING_MAX_LENGTH + 1]); // Too long
        long_string_data.extend_from_slice(&[0x00, 0x00]); // Double null
        let result = SmbiosManager::count_strings_in_record(&long_string_data);
        assert_eq!(result, Err(SmbiosError::StringTooLong));
    }

    #[test]
    fn test_validate_and_count_strings_edge_cases() {
        // Test consecutive nulls in middle (invalid)
        let consecutive_nulls = vec![
            b'H', b'i', 0x00,        // "Hi\0"
            0x00,                     // Second null (creates consecutive nulls)
            b'B', b'y', b'e', 0x00,  // "Bye\0"  
            0x00                      // Final null
        ];
        let result = SmbiosManager::validate_and_count_strings(&consecutive_nulls);
        assert_eq!(result, Err(SmbiosError::InvalidParameter));
        
        // Test minimal valid empty pool
        let empty_pool = vec![0x00, 0x00];
        let count = SmbiosManager::validate_and_count_strings(&empty_pool).unwrap();
        assert_eq!(count, 0);
        
        // Test multiple strings with varying lengths
        let multi_strings = vec![
            b'A', 0x00,                      // "A\0" - 1 char
            b'H', b'e', b'l', b'l', b'o', 0x00, // "Hello\0" - 5 chars
            b'W', b'o', b'r', b'l', b'd', b'!', 0x00, // "World!\0" - 6 chars
            0x00                              // Double null
        ];
        let count = SmbiosManager::validate_and_count_strings(&multi_strings).unwrap();
        assert_eq!(count, 3);
    }

    #[test]
    fn test_string_counting_with_real_bios_record() {
        // Create a realistic BIOS Information record
        let mut bios_info = BiosInformation::new();
        let strings = vec!["Patina Firmware Corp", "v2.5.1-patina", "09/18/2025"];
        
        let record_data = unsafe {
            SmbiosManager::build_record_with_strings(&bios_info.header, &strings).unwrap()
        };
        
        // Count strings in the generated record
        let count = SmbiosManager::count_strings_in_record(&record_data).unwrap();
        assert_eq!(count, 3);
        
        // Verify the record can be added successfully
        let mut manager = SmbiosManager::new(3, 0);
        let handle = manager.add_from_bytes(None, &record_data).unwrap();
        
        // Verify the stored record has correct string count
        let stored_record = manager.records.iter().find(|r| r.header.handle == handle).unwrap();
        assert_eq!(stored_record.string_count, 3);
    }

    #[test]
    fn test_string_counting_performance_characteristics() {
        // Test with maximum allowed string count and lengths
        let mut large_strings = Vec::new();
        for i in 0..20 { // 20 strings
            // Each string is near the maximum length (64 chars)
            let mut string_data = vec![b'A' + (i % 26) as u8; 60]; // 60 'A's, 'B's, etc.
            string_data.push(0x00); // Null terminator
            large_strings.extend_from_slice(&string_data);
        }
        large_strings.push(0x00); // Double null terminator
        
        // This should complete quickly even with many long strings
        let count = SmbiosManager::validate_and_count_strings(&large_strings).unwrap();
        assert_eq!(count, 20);
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
- Record structures use `#[repr(C, packed)]` to maintain SMBIOS specification compliance

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
