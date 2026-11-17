# Patina SBOM Component

The Patina SBOM component collects Software Bill of Materials (SBOM) entries from
pre-DXE firmware and loaded UEFI images. It publishes a combined ACPI SBOM table at
ReadyToBoot so the operating system can discover the firmware's SBOM data.

## Capabilities

- Collects single-entry SBOM GUID HOBs produced before DXE.
- Discovers SBOM protocols associated with loaded images at ReadyToBoot.
- Builds an ACPI SBOM table with platform-provided OEM and creator metadata.
- Publishes the checksummed table through `EFI_ACPI_TABLE_PROTOCOL.InstallAcpiTable`.
- Preserves entry payloads, formats, and compression without interpreting or decompressing them.
- Logs malformed protocol entries and publication failures.

## Components and Services

- **SbomComponent**: Collects SBOM entries and publishes the combined ACPI table.
  Requires `Service<dyn EventServices>` and `Service<dyn ProtocolServices>`, and
  optionally consumes `Hob<SbomHob>`.
- **SbomHob**: Component definition for a GUID HOB containing one serialized `SbomTableEntry`,
  without an ACPI table header.
- **SbomProtocol**: SDK alias for one serialized `SbomTableEntry`, installed directly
  as the protocol interface bytes.
  Patina DXE Core installs this protocol on loaded-image handles when it detects a valid
  `.sbom` section.

This component does not produce a Rust service or install the per-image discovery protocols.

## Configuration

`SbomComponent::new()` accepts the OEM ID, OEM table ID, OEM revision, creator ID,
and creator revision for the generated ACPI table:

```rust
use patina_sbom::component::SbomComponent;

let component = SbomComponent::new(*b"OEMID ", *b"PLATFORM", 1, u32::from_le_bytes(*b"TOOL"), 1);
```

## Platform Integration

### Build Integration

Enable the `unstable-sbom` feature on `patina_sbom`. To collect SBOM data from loaded
images, also enable `unstable-sbom` on `patina_dxe_core`.

The platform must provide `EFI_ACPI_TABLE_PROTOCOL` before ReadyToBoot, using the SDK's
[AcpiTableProtocol definition](../../sdk/patina/src/uefi/acpi_table.rs). The SBOM component
consumes this protocol rather than replacing its implementation, and does not depend on
the `patina_acpi` crate.

### Component Initialization

Add the component to the platform's `ComponentInfo::components()` implementation:

```rust
add.component(patina_sbom::component::SbomComponent::new(
    *b"OEMID ",
    *b"PLATFORM",
    1,
    u32::from_le_bytes(*b"TOOL"),
    1,
));
```

### SBOM Data Sources

Pre-DXE firmware can produce SBOM GUID HOBs using the layout and GUID documented in the
component's [SBOM HOB module](src/hob.rs). Each HOB contains one entry;
multiple HOBs can contribute multiple entries.

Loaded UEFI images can supply an SBOM entry in a `.sbom` PE section. Patina DXE Core
validates the section and publishes the
[SBOM discovery protocol](../../sdk/patina/src/vendor_protocols/sbom.rs) on the image's handle.
Protocol providers retain an immutable entry allocation while installed, containing at least
the fixed ten-byte header and the complete payload declared by its embedded lengths.
The component reads and validates the fixed header, derives the full entry length, and parses
and copies the entry. Providers guarantee the complete declared allocation remains readable;
the protocol does not carry an independent allocation length.

## Table Publication

During dispatch, the component copies the HOB entries and registers a ReadyToBoot notification
at TPL Callback. On the first signal, while Boot Services allocations are available, it collects
the image protocols and deduplicates entries based on their raw bytes before constructing
the table. Malformed protocol entries are logged and skipped.

`InstallAcpiTable` owns the installed copy, allowing the component to release its temporary
construction buffer. No table is published when there are no entries.

Publication is attempted only once. The notification closes its event after the attempt, and
repeated signals are ignored even if publication failed. Discovery, event-registration, and
installation errors are reported. The component does not replace an existing SBOM table or
include entries discovered after its first ReadyToBoot notification.

## Host Tools

The `std` feature enables two standalone host binaries, without changing the firmware library:

```text
cargo make build-lib -p patina_sbom --features std --bins
```

- `sbom-acpi-decode <table.bin>` validates a saved ACPI SBOM table and prints its ACPI header,
  entry headers, and payload bytes in hexadecimal. Payloads retain their original compression.
- `sbom-acpi-dump <output.bin>` captures the running system's SBOM table using the Windows
  firmware-table API or Linux `/sys/firmware/acpi/tables/SBOM`. Linux may require elevated
  read permissions. The tool validates the table before saving and requires a new output path.

For example, after building on Windows:

```text
target\debug\sbom-acpi-dump.exe sbom.bin
target\debug\sbom-acpi-decode.exe sbom.bin
```

Both tools report malformed tables and operating-system errors with a nonzero exit status.
