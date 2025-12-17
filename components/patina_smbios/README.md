# Patina SMBIOS Component

The Patina SMBIOS component provides type-safe SMBIOS table management for
Patina-based firmware. It offers both a modern Rust service API and legacy
C/EDKII protocol compatibility for managing SMBIOS records throughout the
boot process.

## Capabilities

- Produces the `Smbios` service for adding, updating, and publishing SMBIOS
  records with type-safe structured record types.
- Installs the EDKII-compatible `EFI_SMBIOS_PROTOCOL` for C drivers that
  need to interact with SMBIOS tables.
- Manages SMBIOS record handles with automatic allocation, reuse, and PI
  specification compliance.
- Validates and serializes structured SMBIOS records with proper string
  pool handling.
- Publishes SMBIOS tables to the UEFI Configuration Table with proper
  entry point structures and checksums.
- Emits focused log output to aid in debugging record addition, string
  updates, and table publication.

## Components and Services

- **SmbiosProvider component**: Creates the SMBIOS manager, registers the
  `Service<dyn Smbios>`, and installs the C/EDKII protocol. Both interfaces
  share the same underlying manager for consistency.
- **Smbios trait**: Defines core operations (`version`, `publish_table`,
  `update_string`, `remove`, `add_from_bytes`) accessible through trait
  object dynamic dispatch.
- **SmbiosExt trait**: Provides the type-safe `add_record<T>()` generic
  method for adding structured records without manual serialization.

## Configuration

The SMBIOS component requires version configuration during instantiation:

```rust
commands.add_component(SmbiosProvider::new(3, 9));  // SMBIOS 3.9
```

> Only SMBIOS 3.x versions are supported. The component will panic at
> initialization if `SmbiosProvider::new()` is called with an unsupported
> major version (i.e., major version != 3). This is intentional to catch
> configuration errors early during platform development. If the manager
> creation fails during `entry_point()`, the component returns
> `EfiError::Unsupported` instead of panicking.

## Platform Integration

To integrate the `patina_smbios` component into your platform, remove the
`SmbiosDxe` driver and add the `SmbiosProvider` component. This replaces
`SmbiosDxe` as the producer of the `EFI_SMBIOS_PROTOCOL`.

### Step 1: Remove the SmbiosDxe Driver

Remove `SmbiosDxe.inf` from your platform's DSC file (e.g.,
`YourPlatform.dsc` or `YourPlatformCommon.dsc.inc`):

```diff
-  #
-  # SMBIOS Support
-  #
-  MdeModulePkg/Universal/SmbiosDxe/SmbiosDxe.inf {
-    <LibraryClasses>
-      NULL|YourPlatform/Library/SmbiosVersionLib/DetectSmbiosVersionLib.inf
-  }
```

Remove the corresponding entry from your platform's FDF file (e.g.,
`YourPlatform.fdf`):

```diff
-INF  MdeModulePkg/Universal/SmbiosDxe/SmbiosDxe.inf
```

Existing C drivers can locate and use the `EFI_SMBIOS_PROTOCOL` produced by the
Patina SMBIOS component to add their records.

### Step 2: Add the Patina SMBIOS Component

In your Patina DXE core configuration (e.g., `your_platform_dxe_core.rs`), add
the `SmbiosProvider` component:

```rust
use patina_smbios::SmbiosProvider;

// In your DXE core builder
PatinaCore::new()
    .with_component(SmbiosProvider::new(3, 9))  // SMBIOS version 3.9
    // ... other components
```

The version parameters represent the SMBIOS specification version (major, minor).
Only SMBIOS 3.x versions are supported.

### Step 3 (Optional): Create a Platform SMBIOS Component

If you want to develop your own Patina-based platform SMBIOS component, remove
your platform's existing SMBIOS driver (e.g., `SmbiosPlatformDxe`,
`SbsaQemuSmbiosDxe`, `ProcessorSubClassDxe`) from your DSC and FDF files, then
create a platform-specific component that populates your SMBIOS tables. This
component uses the `#[component]` macro and receives the `Smbios` service as a
parameter:

```rust
use patina::component::{component, service::Service};
use patina_smbios::service::{Smbios, SmbiosExt};

#[derive(Default)]
pub struct YourPlatformSmbios;

#[component]
impl YourPlatformSmbios {
    pub fn new() -> Self {
        Self
    }

    fn entry_point(self, smbios: Service<dyn Smbios>) -> Result<()> {
        // Add Type 0 (BIOS Information), Type 1 (System Information), etc.
        // ... populate your platform-specific SMBIOS records

        smbios.publish_table()?;
        Ok(())
    }
}
```

Then add the platform component to your DXE core:

```rust
PatinaCore::new()
    .with_component(SmbiosProvider::new(3, 9))
    .with_component(YourPlatformSmbios::new())
    // ... other components
```

## Integration Guidance

### Adding SMBIOS Records

Platform components should use the type-safe `add_record<T>()` method from `SmbiosExt`:

```rust
use patina::component::service::Service;
use patina_smbios::{
    service::{Smbios, SmbiosExt, SmbiosTableHeader, SMBIOS_HANDLE_PI_RESERVED},
    smbios_record::Type0PlatformFirmwareInformation,
};

fn platform_init(smbios: Service<dyn Smbios>) -> Result<()> {
    // Create a Type 0 BIOS Information record
    let bios_info = Type0PlatformFirmwareInformation {
        header: SmbiosTableHeader::new(0, 0, SMBIOS_HANDLE_PI_RESERVED),
        vendor: 1,
        firmware_version: 2,
        bios_starting_address_segment: 0xE800,
        firmware_release_date: 3,
        firmware_rom_size: 0xFF,
        characteristics: 0x08,
        characteristics_ext1: 0x03,
        characteristics_ext2: 0x03,
        system_bios_major_release: 1,
        system_bios_minor_release: 0,
        embedded_controller_major_release: 0xFF,
        embedded_controller_minor_release: 0xFF,
        extended_bios_rom_size: 0,
        string_pool: vec![
            String::from("Patina Firmware"),
            String::from("1.0.0"),
            String::from("10/31/2025")
        ],
    };

    // Add the record and get the assigned handle
    let handle = smbios.add_record(None, &bios_info)?;
    log::info!("Added BIOS Info record with handle: 0x{:04X}", handle);

    // Publish the table when all records are added
    smbios.publish_table()?;
    Ok(())
}
```

### Available Record Types

The crate provides structured types for common SMBIOS records:

- **Type0PlatformFirmwareInformation**: BIOS/firmware information
- **Type1SystemInformation**: System manufacturer, product name, UUID, SKU
- **Type2BaseboardInformation**: Baseboard/motherboard information
- **Type3SystemEnclosure**: Chassis/enclosure information
- **Type127EndOfTable**: End-of-table marker (required)

Each record type implements `SmbiosRecordStructure` and handles
serialization automatically, including proper string pool conversion and
length calculations.

### String Pool Handling

SMBIOS strings are stored in a `Vec<String>` field marked with
`#[string_pool]`. String indices in record fields are 1-based. The
serialization process automatically converts the string pool to
null-terminated SMBIOS format:

```rust
string_pool: vec![
    String::from("Vendor"),      // Index 1
    String::from("Product"),     // Index 2
    String::from("Version"),     // Index 3
],
```

### Updating Existing Records

Records can be updated after creation using `update_string`:

```rust
// Update string 1 in the record
smbios.update_string(handle, 1, "New Vendor Name")?;
```

### C/EDKII Protocol Compatibility

The component automatically installs the `EFI_SMBIOS_PROTOCOL` with functions:

- `Add`: Add SMBIOS records from raw bytes
- `UpdateString`: Update strings in existing records
- `Remove`: Remove records by handle
- `GetNext`: Iterate through SMBIOS records

C drivers can locate and use this protocol as they would in traditional EDKII firmware.

## Testing

The crate includes comprehensive unit tests demonstrating:

- Record creation and serialization
- Handle allocation and reuse
- String pool validation
- Mock service patterns using `Service::mock()`
- Integration with the service system

Run tests with:

```bash
cargo test --lib
```

## Documentation

For detailed implementation and architectural information, see the inline
documentation and:

- [SMBIOS Specification](https://www.dmtf.org/standards/smbios)
