# Patina ACPI Component

The Patina ACPI component provides a memory-safe interface for managing and interacting with ACPI tables in firmware.
On the Rust side, consumers can directly utilize the `AcpiTableManager` service,
while through FFI the component also produces EDKII compatible C ACPI protocols.

For more information on ACPI in general, see:

- [ACPI Table Protocol](https://uefi.org/specs/UEFI/2.10/20_Protocols_ACPI_Protocols.html) - UEFI Spec 2.10 Vol 20
- [ACPI SDT Protocol](https://uefi.org/specs/PI/1.8/V5_ACPI_System_Desc_Table_Protocol.html) - PI Spec 1.8 Vol 9
- [ACPI Spec 6.5](https://uefi.org/sites/default/files/resources/ACPI_Spec_6_5_Aug29.pdf)

## Capabilities

- Produces the `AcpiTableManager` service for Rust-native interaction with ACPI tables:
install, uninstall, get, iteration, notification callbacks.
- Produces and installs the complete `EFI_ACPI_TABLE_PROTOCOL` and part of `EFI_ACPI_SDT_PROTOCOL`.
  - `GetAcpiTable` and `RegisterNotify` are produced and implemented in Rust.
They are packaged part of a new custom protocol `EFI_ACPI_GET_PROTOCOL`.
- Manages, validates, and publishes ACPI tables.

## Structure

### Services

The `AcpiTableManager` service is the primary service for interacting with ACPI tables in firmware.

Under the hood, it is a wrapper for `AcpiProvider`, which provides the actual ACPI-related functionality.
The `AcpiTableManager` service wrapper allows generic usage of tables; the user can install a table of any type `T`.
**Consumers of the service should not directly interact with `AcpiProvider`.**

### Component

The `AcpiProviderManager` component initializes necessary ACPI structures like the XSDT and RSDP, produces the services
described above, and installs the EDKII-compatible `EFI_ACPI_TABLE_PROTOCOL` and custom `EFI_ACPI_SDT_PROTOCOL`.

### ACPI HOB

Optionally, PEI can produce a HOB of existing ACPI tables. The GUIDed ACPI HOB includes only one data field, the address
of the RSDP. From this, the XSDT and list of existing tables can be gathered.

### C Implementation

The Patina ACPI component implements table management in Rust, but does not implement AML parsing and manipulation.
These are included in the modified `AcpiSdtDxe` driver, which provides the remaining parts of `EFI_ACPI_SDT_PROTOCOL`
(`RegisterNotify`, `Open/OpenSdt`, `Close`, `GetChild`, `Get/SetOption`, `FindPath`).
To interface with the Rust-side tables, these implementations use the custom `EFI_ACPI_GET_PROTOCOL` to retrieve tables
installed by the Patina ACPI component.

## ACPI 2.0 Support

The Patina ACPI component supports ACPI 2.0+ only, with one important caveat.

In order to be compatible with current Windows OS code, the FACS is produced within a 32-bit address range to prevent
overflow in the kernel. This supports both ACPI 1.0 and ACPI 2.0, as both the `firmware_ctrl` (32-bit) and
`x_firmware_ctrl` (64-bit) fields in the FADT are set to the address of the FACS. Code that utilizes the FACS can choose
to read either field.

## Platform Integration

### Component Initialization

To enable the Patina ACPI component during Patina boot, call `AcpiProviderManager::new(...)` with the correct `oem_id`,
`oem_table_id`, `oem_revision`, `creator_id`, and `creator_revision`. These values will vary based on platform.

Then, implement `component` for the core and add the Patina ACPI component:

```rust
add.component(patina_acpi::component::AcpiProviderManager::new( /* Platform config. */ ));
```

### Build Integration

The Patina ACPI component along with the custom `AcpiSdtDxe` C driver replaces the existing EDKII `AcpiTableDxe`
implementation. To integrate the Rust table implementation into your platform, replace all instances of `AcpiTableDxe`
with `AcpiSdtDxe`. `AcpiSdtDxe` should already be included in `patina-qemu` and relevant Intel platforms.

## ACPI Usage

To use the Patina ACPI component, request the `Service<AcpiTableManager>`.

```rust
struct MyComponent {
}

#[component]
impl MyComponent {
    // Note that the Service is NOT `dyn`!
    fn entry_point(acpi_table_service: Service<AcpiTableManager>) {
        // ...
    }
}
```

To install an ACPI table:

```rust
struct MyAcpiTable {
    header: AcpiTableHeader,
    some_value1: u32,
    some_value2: u8,
}

let table_key = acpi_table_service.install_acpi_table(MyAcpiTable { ... });
```

Upon installation, the service will return a `TableKey`. This is an opaque value that can be used to later uninstall or
retrieve the table. Consumers of the service should not attempt to cast or otherwise interact with the `TableKey`.

To uninstall an ACPI table:

```rust
acpi_table_service.uninstall_acpi_table(table_key);
```

Uninstalling a table frees all resources associated with it, and the associated `TableKey` will no longer be usable.

To retrieve an ACPI table:

```rust
let my_acpi_table: MyAcpiTable = acpi_table_service.get_acpi_table::<MyAcpiTable>(table_key);
```

Both `get_acpi_table` and `get_acpi_table_unchecked` require the caller to provide the expected type of the table.
`get_acpi_table` will verify that the type matches the original type at installation, while `get_acpi_table_unchecked`
does not. **Only use `get_acpi_table_unchecked` if you are attempting to cast a table to a known safe type that matches
the structure of the table.**

Consumers of the service **should not** attempt to directly mutate tables retrieved through `get_acpi_table`. This is
**undefined behavior**. To modify a table, first `uninstall` the table, then provide the modified table data to `install`.

Some platforms require certain callbacks to be triggered upon installation of an ACPI table. Use `register_notify` and
`unregister_notify` to construct an `AcpiNotifyFn` to be called when an ACPI table is installed.

`collect_tables` provides all the currently installed tables in a iterable format.

### ACPI Table Format and Safety

`install_acpi_table` is inherently unsafe because it can take any table of type `T`. For it to be safe, the provided
table **must** have a standard ACPI table format, starting with `AcpiTableHeader` (or equivalent bytes). Attempting to
install a table of nonstandard format is **undefined behavior**.

#### Existing ACPI Tables

While consumers of the service can install any ACPI table as long as it follows the safety conventions above, a few
common ACPI table types are implemented in `acpi_table.rs` and ACPI-related constants in `signature.rs`.

## Integration Status

### ACPI Integration

- [X] ACPI feature branch merged into `patina` main.
- [X] ACPI platform testing.
- [ ] ACPI component enabled in `patina-dxe-core-qemu` and `patina-qemu`.
- [ ] ACPI component enabled in Intel physical platforms.

Once ACPI integration is complete, platforms are able to integrate a fully Rust-based **ACPI table** implementation.
However, AML parsing and manipulation are still implemented in C through the custom `AcpiSdtDxe` driver. Although this
is considered an intermediate step towards platform integration of Patina ACPI infrastructure, it is fully functional
and tested, and ready for platforms to adopt.

### AML Integration

The timeline for full ACPI + AML Patina integration is about 3-6 months from completion of ACPI integration.

- [ ] AML design feedback. ([RFC](https://github.com/OpenDevicePartnership/patina/blob/main/docs/src/rfc/text/0020-aml-parser.md))
- [ ] AML implementation.
- [ ] AML platform testing.
- [ ] AML feature branch merged into `patina`.
- [ ] Full Patina AML implementation enabled in `patina-dxe-core-qemu` and `patina-qemu`.
- [ ] Full Patina AML implementation enabled in Intel physical platforms.

Once full AML integration is complete, the entire ACPI + AML system will be based in Rust. The intermediate C driver
`AcpiSdtDxe` will no longer be required.
