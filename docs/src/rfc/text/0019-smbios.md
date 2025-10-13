# RFC: `SMBIOS`

This RFC proposes a Rust-based interface for managing SMBIOS records, safely encapsulating the
`EFI_SMBIOS_PROTOCOL` functionality from the UEFI specification behind an idiomatic Rust service. It
introduces an `SmbiosRecords` trait for adding, updating, removing, and publishing SMBIOS structures.
The design focuses on: (1) preserving required UEFI semantics, (2) reducing memory‑unsound call patterns
inherent in the C protocol, and (3) enforcing a minimal, future‑extensible surface suitable for additional
typed/derive layers. All behavior required for SMBIOS 3.x publication is implemented; experimental typed
record abstractions are deferred to Future Work and an Appendix summarizes only the public API.

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
- 2025-10-08: Added SMBIOS 3.0 Configuration Table publication feature documentation. Documented `install_configuration_table()`
  method, RefCell interior mutability pattern, SMBIOS_3_0_TABLE_GUID, and Smbios30EntryPoint structure. Updated implementation
  status to reflect production-ready table publication to UEFI Configuration Table for OS visibility. Clarified "compatibility"
  definition to explicitly state preservation of UEFI specification semantics while transforming interfaces for Rust integration.

## Motivation

The System Management BIOS (SMBIOS) specification defines data structures and access methods that allow hardware and system
vendors to provide management applications with system hardware information. This RFC proposes a pure Rust interface for
existing SMBIOS capabilities, replacing the C-based implementations while preserving the semantics and behavior specified
in the UEFI specification.

**Compatibility Definition** (consistent with other Patina RFCs):

1. **Functional Equivalence**: The Rust implementation provides all functionality required by `EFI_SMBIOS_PROTOCOL`
2. **Semantic Preservation**: Operations maintain the same behavior, constraints, and error conditions as specified
   in UEFI
3. **Data Format Compliance**: SMBIOS table structures conform exactly to the SMBIOS and UEFI specifications
4. **Interface Transformation**: C-style function pointers and raw pointers are replaced with idiomatic Rust traits,
   ownership, and type safety
5. **Interoperability**: The implementation can produce C-compatible protocol bindings when needed for legacy component integration

This approach provides a simpler, safer Rust-based interface while maintaining required SMBIOS functionality and
ensuring that the resulting firmware behavior aligns with UEFI specification requirements.

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

## SMBIOS Version Support Rationale

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

### Record Ingestion Safety Model

Legacy unsafe ingestion paths (e.g. pointer-based `add()` accepting only a header) were eliminated. The service now
accepts only fully‑formed record byte slices through `add_from_bytes()`, performing length and termination checks
before ownership is assumed. This removal closes an entire class of header length spoof / buffer overrun risks.

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

**Removed Legacy API**: An earlier draft retained an unsafe `add()` for transitional UEFI interop; it has since been
fully removed. No public API now requires `unsafe` for ordinary record submission.

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
    pub minor_version: u8,  // Defaults to 9
}
```

#### Handle Allocation

Uses counter-based sequential allocation with wraparound logic. Skips reserved handles (0, 0xFFFE, 0xFFFF) and
provides O(1) average case performance.

#### Safe API

The primary and only API for adding records is `add_from_bytes()`.

#### String Updates

Provides `update_string()` to modify strings in existing records. Parses the record, updates the targeted string,
and rebuilds the record with proper SMBIOS formatting.

#### Thread Safety

Mutable operations protected by spin::Mutex.

#### Trait Object Safety

Returns `Box<dyn Iterator>` to enable use as `dyn SmbiosRecords<'static>` service.

#### SMBIOS 3.0 Configuration Table Publication

Publishes SMBIOS tables to the OS via `install_configuration_table()`. Uses SMBIOS_3_0_TABLE_GUID
(F2FD1544-9794-4A2C-992E-E5BBCF20E394) and allocates ACPI_RECLAIM_MEMORY for OS visibility. See Appendix for
structure details.

### Not Yet Implemented

The following features are documented in this RFC but not yet implemented:

- Versioned typed record interfaces
- Typed record methods (`add_typed_record()`, `get_typed_record()`, `iter_typed_records()`)
- Forward/backward compatibility with version-aware parsing
- SMBIOS protocol FFI bindings (C ABI extern functions)

These features are described in Future Work sections below.

## Future Work

The current byte-based API is production-ready. Potential future enhancement includes:

- **SMBIOS Protocol FFI Bindings**: C ABI extern functions for legacy UEFI component integration and drop-in
  replacement for existing C-based SMBIOS protocol implementations

## Unresolved Questions

No remaining unresolved questions.

## Prior Art (Existing PI C Implementation)

This Patina-based SMBIOS implementation follows the SMBIOS protocol
as described in the UEFI specification. *See [Protocols](#protocols) for more information.*

In C, `SMBIOS_INSTANCE` provides the core management structure,
`EFI_SMBIOS_ENTRY` represents individual SMBIOS records,
and `SMBIOS_HANDLE_ENTRY` tracks allocated handles.
These are roughly replicated by the Rust structs described in the implementation.

### Dependencies on C Protocols

While the final outcome should be a purely Rust-based interface,
current publication still uses `BootServices.InstallConfigurationTable`.

## Rust Code Design

### Service Overview (Design-Focused)

The SMBIOS component exposes one primary capability: a record service that
collects raw SMBIOS structure byte slices, assigns stable handles, and (on demand)
materializes a 3.0 entry point + packed table for installation into the UEFI
Configuration Table.

Key responsibilities:

- Collect validated raw records (already laid out per spec) and own them until publication.
- Allocate sequential (wrap‑safe) handles (see Handle Allocation summary above).
- Maintain original insertion order for deterministic table emission.
- Build: (a) contiguous structure table with double‑null termination, (b) 24‑byte 3.0 entry point with checksum.
- Publish exactly once (idempotent guard) to avoid duplicate configuration table entries.

Deliberately excluded from the design section (moved to Appendix / Future Work):

- Typed / version‑aware record abstractions
- Generic (derive-based) serializers / reflection helpers
- Forward‑compat parsing helpers
- FFI protocol scaffolding details
- Internal manager / RefCell layout and non‑public structs

### Public Interaction Pattern (High Level)

1. Create manager with target (major, minor) version.
2. Add records via a safe method taking a pre-built byte slice; service returns allocated handle.
3. Optionally adjust strings (limited, safe replacement) before publication.
4. Call publish → returns (entry_point_phys, table_phys); after this records become immutable.

Error surfaces (design‑relevant):

- Invalid record buffer (too short for header / malformed termination)
- Out of handles (exhaustion after wrap)
- String index out of range on update

### Safety Boundary

All structural interpretation (e.g. typed views, speculative future parsing) is explicitly out of scope
for the minimal design and will live in optional layers. The core service treats each record as an opaque
validated blob after initial length / termination checks.

### Future Extensions (Pointers Only)

See Future Work for: typed records, derive-based serializers, forward/backward compatibility helpers.

## Guide-Level Explanation

A component obtains the service (e.g. through dependency injection / locator), submits raw SMBIOS record
byte slices, then triggers publication. The consumer does not manipulate internal tables or handles directly.

(Full usage examples and extensive code have been removed for brevity and can be reconstructed from the
Appendix if needed.)

## Appendix A: Public API Summary

This appendix lists only the externally visible, intentionally supported surface. Internal helper types
(allocators, inner structs, reflection/derive prototypes, FFI glue) are intentionally excluded.

### Configuration

```rust
pub struct SmbiosConfiguration {
    pub major_version: u8; // default 3
    pub minor_version: u8; // default 0
}
```

Used by platform integration to choose target SMBIOS spec revision (3.x only).

### Core Types

```rust
pub type SmbiosHandle = u16; // 0, 0xFFFE, 0xFFFF reserved

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmbiosError {
    InvalidParameter,
    BufferTooSmall,
    HandleNotFound,
    OutOfHandles,
    StringIndexOutOfRange,
}
```

### Service Trait (Stable Surface)

```rust
pub trait SmbiosRecords {
    fn add_from_bytes(&self, producer: Option<Handle>, record: &[u8]) -> Result<SmbiosHandle, SmbiosError>;
    fn update_string(&self, handle: SmbiosHandle, string_number: usize, new_value: &str) -> Result<(), SmbiosError>;
    fn install_configuration_table(&self) -> Result<(u64, u64), SmbiosError>; // (entry_point_phys, table_phys)
    fn version(&self) -> (u8, u8);
}
```

Notes:

- Unsafe legacy `add()` intentionally omitted here (legacy UEFI interop only).
- Publication is idempotent; subsequent calls return the same addresses (future: may add AlreadyPublished error).

### Publication

Service constructs the 24‑byte SMBIOS 3.0 entry point with checksum and installs it plus the packed table into the
UEFI Configuration Table using `SMBIOS_3_0_TABLE_GUID` (F2FD1544-9794-4A2C-992E-E5BBCF20E394).

### Minimal Usage Sketch

```rust
fn example(records: &dyn SmbiosRecords) -> Result<(), SmbiosError> {
    let h = records.add_from_bytes(None, &type0_bytes)?;
    records.update_string(h, 1, "Acme BIOS 1.2.3")?;
    let (_ep, _table) = records.install_configuration_table()?; // OS-visible
    Ok(())
}
```
