# RFC: UEFI Services Component

This RFC proposes a new component that will abstract "UEFI Services" within the component model.

Within the scope of this RFC, "UEFI Services" include:

- UEFI Boot Services
- UEFI Runtime Services

The services within the scope of this crate include those as defined in the [UEFI Specification](https://uefi.org/specifications)
and not those in the [Platform Initialization (PI) Specification](https://uefi.org/specifications). Services outside
of the UEFI specification, such as those in the PI specification, can follow a similar pattern to what is proposed
here, but are not in scope for this RFC.

## Change Log

- 2025-07-16: Initial draft of RFC.
- 2025-07-16: Added "Requirements" section.
- 2025-07-17: Update the RFC to state that the pre-existing [Memory Management service](https://github.com/OpenDevicePartnership/patina/blob/55fcb7704b6917d7ccb9744dd5bedeaa261af5c4/docs/src/rfc/text/0002-memory-management-interface.md)
  should be used for memory management services in components.
- 2025-09-22: Updated RFC to add proposed groups of UEFI services (addressing a previously unresolved question),
  and adding layout detail for the `patina_uefi_services` crate. Also add a usage example showing how the services
  can be used as injected dependencies in a component.
- 2025-10-1: Update the section about Patina component and C driver dispatch to reflect recent changes that interleave
  component and C driver dispatch.
-2025-10-1: Add mocking note to service interfaces section.

## Motivation

Patina components are firmware and operate for the forseeable future in a DXE environment that will contain a large
number of C authored modules. As firmware that coexists in the DXE environment with these C modules, Patina components
need to be able to interact through the binary interfaces shared with these C modules. For example, to use the protocol
services to install or locate a protocol or the event services to register or signal an event. While many of the core
Boot Services (and some Runtime Services) are implemented in Rust, it is beneficial to abstract Pure Rust Patina
components from C-based constructs such as the services tables to:

1. Align with the Patina service model for consistency with other services provided by components.
2. Abstract interfaces so they can be used in a more idiomatic way in Rust. The flexibility is available to modify
   the function signature in the service trait to be more idiomatic, such as using using a custom status code instead
   of `EFI_STATUS`.
3. Better associate granular dependencies within Patina components on specific UEFI services. For example, today a
   component uses "Boot Services" as a dependency, but it may only use a small subset of the Boot Services. By
   abstracting the services, a component can depend on only the specific services it uses and that can be tracked in
   dependency graphs and audit tools.
4. Detach from monolithic service table dependencies so services can potentially be deprecated in the future. For
   example, Patina may introduce a new "UEFI Variable" interface that is safer and more ergonomic that those in the
   UEFI Runtime Services table. For example, this interface may include built-in support for UEFI Variable Policy.
   We would rather components use this service and entirely forbid usage of the legacy variable interface.
5. Support earlier dispatch of components. Today, the service table pointers are stored in component storage:

   ```rust
        unsafe {
            self.storage.set_boot_services(StandardBootServices::new(&*boot_services_ptr));
            self.storage.set_runtime_services(StandardRuntimeServices::new(&*runtime_services_ptr));
        }
   ```

   Components can be dispatched prior to a C driver that produces functionality that the component depends on. For
   example, services such as variable services in the Runtime Services table are not currently available until after
   the UEFI variable C driver has dispatched and updated the pointers for variable functions in the Runtime Services
   table. If a "UEFI Variable Service" is provided, then the component can depend on that service and be dispatched at
   the proper time. If that service moves to Rust, there is no change needed in the component code, it continues to
   depend on the "UEFI Variable Service" and dispatch when it is available.

## Technology Background

Patina components (and constituent elements such as "services) are primarily described in
[Monolithically Compiled Components](https://github.com/OpenDevicePartnership/patina/blob/main/docs/src/component/interface.md).

## Goals

1. Treat UEFI Specification defined services as "component services".
2. Introduce a Patina component service abstraction layer for interface flexibility and ergonomic component usage.
   > Note: The abstraction layer also presents an opportunity to instrument telemetry into individual service function
   > usage such as tracking how often signal_event is called and from which component the call originated.
3. Make services in UEFI Specification defined service tables more granular to:
   1. Participate more precisely in the component dependency graph.
   2. Better track with auditing and dependency analysis tools. For example, counting how many component depend on
      event services.

## Requirements

> Note: "Boot Services" and "Runtime Services" in the UEFI Specification are generically referred to as "UEFI Services"
> in this RFC.

1. Make a component crate available called `patina_uefi_services` that provides "UEFI Services" to Patina components
   that do not have an equivalent services produced today. At this time, that excludes "Memory Services" provided by
   the [`MemoryManager` service](https://github.com/OpenDevicePartnership/patina/blob/728c7e3a345a0a74351b14c1ff9a6bf948248fed/patina_dxe_core/src/memory_manager.rs#L27).
2. All Boot Services and Runtime Services must be accounted for in the `patina_uefi_services` component crate unless
   exempted by (1).
3. The `patina_uefi_services` component crate must not provide any service outside of those within Boot Services and
   Runtime Services (at this time). In the future, it may be allowable to include other APIs defined in the UEFI
   Specification as services.
4. Patina components must use the `patina_uefi_services` component crate to access any "UEFI Service" offered by
   services produced by the component. Components must not directly access the service tables.
5. Services must be grouped by logical functionality into separate trait definitions to support fine-grained dependency
   management.
6. Multiple provider components must be available to allow flexible integration patterns, including both a comprehensive
   provider and individual service providers.
7. Do not make policy decisions in these services. These services are not intended to enforce decisions like whether
   a component *should* use protocols. It simply provides a Patina services path for components that *need* to use
   protocols.

## Unresolved Questions

- What to name the service traits and how to handle them changing in the future.
  - The current proposal is to offer service APIs that are very similar in semantics (function signatures might not
    match exactly) to the spec-defined interfaces. This interfaces will not be opinionated but simply provide native
    Patina service abstractions for the UEFI services.
  - If in the future, we decide to make a different interface, these services remain close to the specification, and
    new service traits can be introduced with different names and the new interfaces.
  - To be clear, these service traits can accept more "Rust-like" types like `&str` instead of `*const CHAR16` for
    string parameters, and return `Result<T>` instead of `EFI_STATUS`. But, they should semantically remain the same.
    For example, continue to honor TPL levels even if a different "prioritization" service crate is available that
    uses a different prioritization scheme.

- Whether to have a single "provider" component that provides all UEFI services or multiple provider components.
  - The current proposal is to have both a comprehensive provider component that provides all UEFI services and
    individual provider components for each service group. This has more flexibility for platform integrators.
  - For example, a simple system may use the comprehensive provider component to make all UEFI services available
    to components. Firmware that needs only a small subset of services to only provide certain UEFI services by
    including only the individual provider components needed.

    > - Note: The comprehensive provider component is not "separate". It can be implemented as a composition of the
    > individual provider components to avoid code duplication.

## Prior Art (Existing PI C Implementation)

The prior art is to use `StandardBootServices` and `StandardRuntimeServices` as the service tables as provided to
component storage and demonstated in the "Alternatives" section below.

## Alternatives

Allow components to directly consume Boot Services and Runtime Services tables. This takes the service table as a
monolithic dependency for the component via dependency injection. For example:

```rust
pub fn entry_point(
    self,
    boot_services: StandardBootServices,
    runtime_services: StandardRuntimeServices,
) -> Result<(), EfiError> {
```

This is what is available today. The component then uses the service table directly, such as:

```rust
boot_services.as_ref().create_event_ex(
    EventType::NOTIFY_SIGNAL,
    Tpl::CALLBACK,
    Some(event_callback::callback_fn),
    Box::new((BB::clone(&boot_services), RR::clone(&runtime_services), fbpt)),
    &EVENT_GROUP_END_OF_DXE,
)?;
```

This is undesirable because the dependencies are not granular, the dependencies are not "true Patina services", and
those traits are more restrained to align with the C function interfaces than a separate Patina abstraction would be.

## Rust Code Design

A `patina_uefi_services` crate provides a Patina service abstraction layer to "UEFI services" so they can be used by
Patina components.

Services are logically grouped by functionality:

- `ConsoleServices` - Console input/output operations
- `EventServices` - Event and timer management
- `ImageServices` - Image handling operations
- **Miscellaneous**
  - `ConfigurationTableServices` - Install & uninstall configuration tables
  - `CrcServices` - CRC calculation helpers
  - `MemoryUtilityServices` - Generic memory operations like copy memory and set memory
  - `MonotonicCounterServices` - Services for monotonic counters
  - `TimingServices` - Basic stall and watchdog timer services
- `ProtocolServices` - Protocol management
- `RuntimeTimeServices` - Time services from Runtime Services
- `RuntimeResetServices` -  Reset services from Runtime Services
- `RuntimeVariableServices` - UEFI variable services

### Crate Structure

Following established Patina component structure, the `patina_uefi_services` crate would primarily be implemented in
two high-level modules:

- `service/` - Contains trait definitions for all UEFI service abstractions
- `component/` - Contains concrete implementations and provider components

### Service Trait Organization

Services are logically grouped by functionality into separate traits so:

1. It is clear when looking at the dependency list for a component what its logical dependencies are. For example, does
   it access UEFI variables? Does it potentially reset the system? This is more granular than the spec-defined service
   tables.
2. It reduces the API impact surface if an interface in a service group is modified. For example, changes to the UEFI
   variable interfaces would only impact components using UEFI variable services.
3. The services have higher cohesion and are easier to reason about and maintain.

#### Console Services (`service::console::ConsoleServices`)

Provides console input/output operations:

- `clear_screen()` - Clear console display
- `enable_cursor()` - Control cursor visibility
- `is_key_available()` - Check for available input
- `output_string()` - Write text to console
- `query_mode()` - Gets current console mode information
- `read_key_stroke()` - Read keyboard input
- `reset_input()` - Reset the console input buffer
- `set_cursor_position()` / `get_cursor_position()` - Cursor management

#### Event Services (`service::event::EventServices`)

Manages UEFI events and timers:

- `check_event()` - Check event status
- `close_event()` - Cleanup events
- `create_event()` - Create new events
- `set_timer()` - Configure timer events
- `signal_event()` - Signal events
- `wait_for_event()` - Wait for event signaling

#### Image Services (`service::image::ImageServices`)

Manages image loading and execution:

- `exit()` - Clean image exit
- `load_image()` - Load executable images
- `start_image()` - Execute loaded images
- `unload_image()` - Unload images

#### Miscellaneous Services (`service::misc`)

- `service::misc::ConfigurationTableServices` - Install & uninstall configuration tables
  - `install_configuration_table()` - Add configuration tables
  - `uninstall_configuration_table()` - Remove configuration tables
- `service::misc::CrcServices` - CRC calculation helpers
  - `calculate_crc32()` - Compute CRC32 checksums
- `service::misc::MemoryUtilityServices` - Memory utility functions
  - `copy_memory()` - Copy memory regions
  - `set_memory()` - Set memory regions to a value
- `service::misc::MonotonicCounterServices` - Monotonic counter operations
  - `get_next_monotonic_count()` - Get the next monotonic count
- `service::misc::TimingServices` - Timing and delay operations
  - `set_watchdog_timer()` - Configure the watchdog timer
  - `stall()` - Busy-wait for a specified duration

#### Protocol Services (`service::protocol::ProtocolServices`)

Handles UEFI protocol management:

- `install_protocol_interface()` - Install protocols on handles
- `uninstall_protocol_interface()` - Remove protocols
- `reinstall_protocol_interface()` - Update protocol interfaces
- `locate_protocol()` - Find protocol instances
- `locate_handle_buffer()` - Find handles with protocols
- `open_protocol()` / `close_protocol()` - Protocol access management

#### Runtime Variable Services

Two separate traits for different variable access patterns:

- `service::runtime::RuntimeVariableServices` - Runtime services table access
  - `get_variable()` - Retrieve UEFI variables
  - `get_next_variable_name()` - Enumerate UEFI variable names
  - `query_variable_info()` - Query variable information
  - `set_variable()` - Set UEFI variables

#### Runtime Time Services

Additional runtime service abstractions:

- `service::runtime::RuntimeTimeServices` - Time management
  - `get_time()` - Get current time
  - `get_wakeup_time()` - Get wakeup time settings
  - `set_time()` - Set current time
  - `set_wakeup_time()` - Set wakeup time settings

#### Runtime Reset Services

- `service::runtime::RuntimeResetServices` - System reset operations
  - `reset_system()` - Perform system resets

### Usage Pattern

Components consume UEFI services through normal service dependency injection:

```rust
use patina_sdk::component::{IntoComponent, prelude::Service};
use patina_uefi_services::service::console::ConsoleServices;
use patina_sdk::error::Result;

#[derive(IntoComponent)]
struct MyComponent;

impl MyComponent {
    fn entry_point(
        self,
        console: Service<dyn ConsoleServices>
    ) -> Result<()> {
        console.clear_screen()?;
        console.output_string("Hello from Patina!")?;
        console.set_cursor_position(10, 5)?;
        console.output_string("Positioned text!")?;
        Ok(())
    }
}
```

> Note: Mocking support via [`mockall`](https://docs.rs/mockall/latest/mockall/index.html) will be implemented on
> service interfaces.

## Guide-Level Explanation

### For Component Authors

The `patina_uefi_services` crate enables Patina components to access UEFI functionality through safe, idiomatic Rust
interfaces. Instead of working directly with C-style UEFI service tables, components can depend on specific services
that provide only the functionality they need.

#### Basic Usage

To use UEFI services in a component, add the relevant service as a dependency in the component's entry point:

```rust
use patina_sdk::component::{IntoComponent, prelude::Service};
use patina_uefi_services::service::{
    console::ConsoleServices,
    event::EventServices,
    protocol::ProtocolServices,
};
use patina_sdk::error::Result;

#[derive(IntoComponent)]
struct MyFirmwareComponent;

impl MyFirmwareComponent {
    fn entry_point(
        self,
        console_services: Service<dyn ConsoleServices>,
        event_services: Service<dyn EventServices>,
    ) -> Result<()> {
        // Use console services
        console_services.clear_screen()?;
        console_services.output_string("Initializing my firmware component...")?;

        // Create an event
        let my_event = event_services.create_event(
            EventType::NOTIFY_SIGNAL,
            Tpl::CALLBACK
        )?;

        console_services.output_string("Component initialization complete!")?;
        Ok(())
    }
}
```

#### Integration Setup

To make UEFI services available in a given Patina binary build, register the provider components during core
initialization:

```rust
use patina_uefi_services::{
    UefiServicesProvider,
    RuntimeVariableServicesProvider,
    ConsoleServicesProvider,
};

// Option 1: Use the single provider that installs all UEFI services
let complete_uefi_services_provider = UefiServicesProvider;
core.with_component(complete_uefi_services_provider);

// Option 2: Use individual providers to reduce the number of unnecessary services and their availability
// (useful if only a few services are needed and/or the platform wants to restrict certain services like protocols)
let variable_services_provider = RuntimeVariableServicesProvider;
let console_provider = ConsoleServicesProvider;
core.with_component(variable_services_provider);
core.with_component(console_provider);
```
