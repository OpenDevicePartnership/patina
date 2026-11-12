# Patina Samples Crate

This crate contains sample components that demonstrate how to build or use other Patina components. These example
components are meant to serve as references for developers looking to create their own Patina components or understand
and modify working component examples when developing new code.

The crate is `no_std` by default and has coverage instrumentation disabled.

## Contents

### Basic Components

Located in `component/hello_world.rs`.

- `HelloStruct`: A struct-based component that uses the default entry point and reads a `Config<i32>` value.
- `GreetingsEnum`: An enum-based component with a custom entry point.

### UEFI Services Samples

Located in `component/uefi_services/`. These show how to use the Patina UEFI Services from
`patina::component::service::uefi_services`. `UefiServicesSample` is re-exported from the module root.

| Module                                  | Description                                                         |
|-----------------------------------------|---------------------------------------------------------------------|
| `overview`                              | A quick tour of timer, event, and protocol usage in one component.  |
| `component_name_producer`               | Produces the component name protocols for a driver.                 |
| `configuration_table`                   | Installs a vendor configuration table and reads it back.            |
| `driver_connect`                        | Discovers controllers by protocol and connects drivers to them.     |
| `driver_binding_producer`               | Manages controllers through the UEFI Driver Model.                  |
| `driver_supported_efi_version_producer` | Produces the driver supported EFI version protocol.                 |
| `end_of_dxe_protocol_consumer`          | Defers protocol consumption to End-of-DXE.                          |
| `media_watcher`                         | Updates per-controller driver state from a timer callback.          |
| `protocol_consumer`                     | Shows different ways to consume a protocol.                         |
| `protocol_dependency`                   | Gates component dispatch on a protocol using `Protocol<P>`.         |
| `protocol_publisher`                    | Publishes a protocol from one component and consumes it in another. |
| `timers`                                | Drives work from one-shot and periodic timers using Rust closures.  |
| `tpl_critical_section`                  | Serializes access to shared state with TPL.                         |

### SMBIOS Platform Sample

Located in `smbios_platform.rs`. `SmbiosExampleComponent` shows the recommended SMBIOS integration pattern with
`patina_smbios`, adding records through the type-safe `add_record<T>()` API. It adds Type 0 (platform firmware),
Type 1 (system), Type 2 (baseboard), and Type 3 (system enclosure) records, plus a custom vendor-specific Type 0x80
record (`VendorOemRecord`) defined with the `SmbiosRecord` derive macro. It is meant to be registered alongside the
`patina_smbios` provider and protocol publisher components.
