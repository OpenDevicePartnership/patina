# RFC: `<Title>`

One paragraph description of the RFC.

## Change Log

- 2025-05-30: Initial RFC created.

## Motivation

As part of the goal to move to a pure Rust interface for performance, we require a Rust-based implementation that installs the FBPT and FPDT, deals with events related to the end of DXE and ExitBootServices, and registers FPDT status code listeners.
At the time of this RFC, such functionality does not exist in the performance component.

## Technology Background

Firmware performance is recorded by two main ACPI tables: the FBPT and FPDT.

The FBPT captures basic boot performance data early in the boot process, and is usually passed to DXE in the HOB. It includes basic information like the reset-end and OS loader timing.

The FPDT captures more fine-grained performance data. It includes both boot records and runtime events, and is created and installed during DXE.

At the time of this RFC, we do not support the S3PT.

## Goals

The overall goal of this new performance component is to eliminate all dependencies on C performance libraries. The new `FirmwarePerformanceDxe` component will cover remaining performance functionality, including:

1. Publishing the FBPT when performance tracing is disabled
2. Installing the FPDT when performance tracing is enabled
3. Collecting early boot performance metrics for the FBPT: reset-end and OS loader timestamps
4. Recording timestamps of `ExitBootServices` for the FPDT and FBPT
5. Registering FPDT status code listeners

## Unresolved Questions

- How do we avoid the use of global statics, given that many of the C-based APIs provide limited ways to pass around context?
- What is the minimal amount of `unsafe` code we need to work with the C-based APIs for events, status codes, and variable services?

## Prior Art (Existing PI C Implementation)

The Rust `FirmwarePerformanceDxe` component closely follows the design of the `FirmwarePerformanceDxe` driver in C.

The primary functionality of the C `FirmwarePerformancedxe` driver is encapsulated in the `FirmwarePerformanceDxeEntryPoint`, which registers the necessary FPDT status code listener and sets up two event callbacks for `EndOfDxe` and `ExitBootServices`.

The `EndOfDxe` FPDT event installs the FPDT if performance is enabled. The `ExitBootServices` event records the entry and exit times of `ExitBootServices`, and unregisters the FPDT status code listener.

For the FBPT, the driver fills in recorded event times and installs the table, regardless of whether performance is enabled.

### Dependencies on C Protocols/Services

At the time of this RFC, performance has several C dependencies. Even if the `FirmwarePerformanceDxe` component is implemented in Rust, we still have unresolved dependencies on C protocols and services, including:

- Runtime services (for variable storage and retrieval across boot)
- Status code protocol (for registering a new FPDT listener)
- Boot services (for eventing)

To achieve a pure Rust implementation of performance, the above features will also have to be re-implemented in Rust.

## Rust Code Design

### `FirmwareDxeComponent`: Public Interface

The primary public interface will be the `FirmwareDxeComponent` component, which will be initialized and configured as part of DXE dispatch.

`FirmwarePerformanceDxeInit` provides configuration for initialization:

```rust
struct FirmwarePerformanceDxeInit {
    oem_id: String,
    oem_table_id: u64,
    oem_revision: u32,
    creator_id: u32,
    creator_revision: u32,
}
```

### `FirmwareDxeComponent`: Private Internals

While not available to consumers of the component, `FirmwareDxeComponent` internally holds data necessary to implement its functionality.

```rust
pub struct FirmwarePerformanceDxe<B: BootServices + 'static, R: RuntimeServices + 'static> {
    /// Internal storage for the FBPT
    boot_performance_table: Mutex<BootPerformanceTable>,

    /// Internal storage for the FPDT
    firmware_performance_table: Mutex<FirmwarePerformanceTable>,

    /// Provides memory allocation services
    /// Primarily used to install the FPDT
    memory_manager: OnceCell<Service<dyn MemoryManager>>,

    /// Used for registering EndOfDxe and ExitBootServices events
    boot_services: OnceCell<B>,

    /// Used for variable retrieval and storage
    runtime_services: OnceCell<R>,
}
```

The use of `Mutex` and `OnceCell` allow synchronization of shared tables, but also interior mutability, since the component will not be fully initialized until its `entry_point` is triggered with the correct configuration.

### Performance Tables

The FPDT and FBPT are represented by the `FirmwarePerformanceTable` and `BootPerformanceTable` structs respectively.

```rust
struct BootPerformanceTable {
    header: AcpiFpdtPerformanceTableHeader,
    basic_boot_record: FirmwareBasicBootPerfDataRecord,
}

struct AcpiFpdtPerformanceTableHeader {
    signature: u32,
    length: u32,
}

pub struct FirmwareBasicBootPerfDataRecord {
    /// Timer value logged at the beginning of firmware image execution 
    pub reset_end: u64,

    /// Timer value logged just prior to loading the OS boot loader into memory
    pub os_loader_load_image_start: u64,

    /// Timer value logged just prior to launching the currently loaded OS boot loader image
    pub os_loader_start_image_start: u64,

    /// Timer value logged at the point when the OS loader calls the ExitBootServices function for UEFI compatible firmware
    pub exit_boot_services_entry: u64,

    /// Timer value logged at the point just prior to the OS loader gaining control back from the ExitBootServices function for UEFI-compatible firmware
    pub exit_boot_services_exit: u64,
}
```

```rust
struct FirmwarePerformanceTable {
    header: AcpiDescriptionHeader,
    basic_boot_record: AcpiFpdtBootPerformanceTablePointerRecord,
}

/// Basic ACPI header - shared by all ACPI tables except the FACS
struct AcpiDescriptionHeader {
    signature: u32,
    length: u32,
    revision: u8,
    checksum: u8,
    oem_id: [u8; 6],
    oem_table_id: [u8; 8],
    oem_revision: u32,
    creator_id: u32,
    creator_revision: u32,
}

struct AcpiFpdtBootPerformanceTablePointerRecord {
    header: AcpiFpdtPerformanceRecordHeader,
    reserved: u32,
    /// Pointer to the basic boot performance record table
    boot_performance_table_header: u64,
}

struct AcpiFpdtPerformanceRecordHeader {
    /// Determines the format and contents of the performance record
    record_type: u16,

    /// Length of the performance record
    length: u8,

    ///  Updated if the format of the record type is extended
    revision: u8,
}
```

### Callbacks

The `FirmwareDxeComponent` registers three callbacks:

1. `fpdt_status_code_listener_dxe`: a status code listener for the FPDT
2. `fpdt_end_of_dxe_event_notify`: an `EndOfDxe` event which installs the FPDT
3. `fpdt_exit_boot_services_event_notify` an `ExitBootServices` event which records the timing of `ExitBootServices` and unregisters the FPDT status code listener

### Context

Due to the required event API `fn<T>(_event: efi::Event, context: Box<T>)` and the undesirability of using global statics, we use the `context` argument to store necessary context for callbacks.

An example for `fpdt_notify_end_of_dxe` is provided below.

```rust
struct FpdtContext<R>
where
    R: RuntimeServices,
{
    firmware_table: *const spin::Mutex<FirmwarePerformanceTable>,
    boot_table: *const spin::Mutex<BootPerformanceTable>,
    memory_manager: Service<dyn MemoryManager>,
    runtime_services: *const R,
}
```

The callback must own the passed-in context, but the inner variables (like `firmware_table`) are used inside the entry point and across multiple callbacks, so we give references and dereference unsafely instead of passing in the owned objects directly.

```rust
let ctx = Box::new(FpdtContext {
    firmware_table: &self.firmware_performance_table,
    boot_table: &self.boot_performance_table,
    memory_manager,
    runtime_services: &*self.runtime_services.get().unwrap(),
});
```

When creating an event, boot services the context as a single struct for simplicity.

```rust
self.boot_services.get().unwrap().create_event_ex(
    EventType::NOTIFY_SIGNAL,
    Tpl::CALLBACK,
    Some(fpdt_notify_end_of_dxe),
    ctx,
    &EVENT_GROUP_END_OF_DXE,
)?;
```

For testability and readability, the event callback is simply a wrapper around a Rust function. As such, after unpacking the context, we are again able to work in pure Rust without raw pointers:

```rust
extern "efiapi" fn fpdt_notify_end_of_dxe<R>(_event: efi::Event, context: Box<FpdtContext<R>>)
where
    R: RuntimeServices,
{
    // Destructure context
    let (firmware_performance_table, boot_performance_table, memory_manager, runtime_services) =
        (context.firmware_table, context.boot_table, context.memory_manager, context.runtime_services);

    let fw_mutex: &spin::Mutex<FirmwarePerformanceTable> = unsafe { &*firmware_performance_table };
    let bp_mutex: &spin::Mutex<BootPerformanceTable> = unsafe { &*boot_performance_table };

    // Retrieve the underlying tables from the locked references
    let mut fw = fw_mutex.lock();
    let mut bp = bp_mutex.lock();

    // Call native Rust function without raw pointers
    let _ = install_firmware_performance_data_table(&mut fw, &mut bp, memory_manager, unsafe { &*runtime_services });
}

fn install_firmware_performance_data_table<R>(
    firmware_performance_table: &mut FirmwarePerformanceTable,
    boot_performance_table: &mut BootPerformanceTable,
    memory_manager: Service<dyn MemoryManager>,
    runtime_services: &R,
) -> Result<(), FirmwarePerformanceError>
where
    R: RuntimeServices,
{
    // Receives unpacked context and installs the FPDT
    // ...
}
```

### Status Code Data

In a similar manner as above, we avoid using global statics for the status code listener callback by adding "hidden" trailing data to the object passed into the status code callback `fpdt_status_code_listener_dxe`,

```rust
#[repr(C)]
pub struct StatusCodeData {
    pub header_size: u16,
    pub size: u16,
    pub status_code_type: efi::Guid,
    // Extra data that won't be seen by the C code, but can be used in the Rust implementation
    boot_table: *const spin::Mutex<BootPerformanceTable>,
}

extern "efiapi" fn fpdt_status_code_listener_dxe(
    code_type: StatusCodeType,
    value: StatusCodeValue,
    instance: u32,
    guid: *const r_efi::efi::Guid,
    data: *const StatusCodeData,
) -> efi::Status {
    // Extract the status code data from the raw pointer
    let status_code_data: &StatusCodeData = unsafe { &*data };

    // Read the "hidden" field 
    let boot_mutex: &spin::Mutex<BootPerformanceTable> = unsafe { &*status_code_data.boot_table };

    // Operate on the FBPT as necessary
    // ...
}
```

## Guide-Level Explanation

Again, the `FirmwarePerformanceDxe` component has little visibility to a consumer, since its primary job is initialization and setup of various performance structures.

It can be hooked into dispatch as follows:

```rust
let firmware_performance_dxe_config = FirmwarePerformanceDxeInit::default();

Core::default()
    // ...
    .with_component(FirmwarePerformanceDxe) 
    .with_config(firmware_performance_dxe_config)
    // ...
```

Values in the `FirmwarePerformanceDxeInit` configuration come from the platform, replacing PCD values used in C.

Besides dispatch, `FirmwarePerformanceDxe` does not provide any consumable protocols or services.
