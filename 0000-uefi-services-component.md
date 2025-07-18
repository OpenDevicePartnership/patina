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

## Motivation

Patina components are firmware and operate for the forseeable future in a DXE environment that will contain a large
number of C authored modules. As firmware that coexists in the DXE environment with these C modules, Patina components
need to be able to interact through the binary interfaces shared with these C modules. For example, to use the protocol
services to install or locate a protocol or the event services to register or signal an event. While many of the core
Boot Services (and some Runtime Services) are implemented in Rust, it is beneficial to abstract Pure Rust components
from C-based constructs such as the services tables to:

1. Align with the Patina service model for consistency with other services provided by components. In a sense, this
   supports the "treat core as a component" approach that has been discussed in the past.
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

   Components are also dispatched prior to C drivers. However, some subsets of services such as variable services in
   the Runtime Services table are not available until after the C drivers are dispatched and the C driver has updated
   the pointers for variable functions in the Runtime Services table. If instead, a "UEFI Variable Service" is provided,
   then the component can depend on that service and be dispatched at the proper time. If that service moves to Rust,
   there is no change needed in the component code, it continues to depend on the "UEFI Variable Service" and dispatch
   when it is available.

## Technology Background

Patina components (and constituent elements such as "services) are primarily described in
[Monolithically Compiled Components](https://github.com/OpenDevicePartnership/patina/blob/main/docs/src/component/interface.md).

## Goals

1. Treat UEFI Specification defined services as "component services".
2. Introduce a Patina component service abstraction layer for interface flexibility and ergonomic component usage.
   > Note: The abstraction layer also presents an opportunity to instrument telemetry into individual service function
   usage such as tracking how often signal_event is called and from which component the call originated.
3. Make services in UEFI Specification defined service tables more granular to:
   1. Participate more precisely in the component dependency graph.
   2. Better track with auditing and dependency analysis tools. For example, counting how many component depend on
      event services.

## Requirements

> Note: "Boot Services" and "Runtime Services" in the UEFI Specification are generically referred to as "UEFI Services"
in this RFC.

1. Make a single component available called `patina_uefi_services` that provides "UEFI Services" to Patina components
   that do not have an equivalent services produced today. At this time, that is only expected to apply to
   "Memory Services" provided by the [`MemoryManager` service](https://github.com/OpenDevicePartnership/patina/blob/728c7e3a345a0a74351b14c1ff9a6bf948248fed/patina_dxe_core/src/memory_manager.rs#L27).
2. All Boot Services and Runtime Services must be accounted for in the `patina_uefi_services` component unless exempted
   by (1).
3. The `patina_uefi_services` component must not provide any service outside of those within Boot Services and Runtime
   Services (at this time). In the future, it may be allowable to include other APIs defined in the UEFI Specification
   as services.
4. Patina components must use the `patina_uefi_services` component to access any "UEFI Service" offered by services
   produced by the component.

## Unresolved Questions

- Where exactly to draw lines between services?
- Whether to produce various services from a single component (A) or multiple components (B)?
  - For example:
    - A:
      - Component: `patina_uefi_services`
        - Produces:
          - `EventService`
          - `ProtocolService`
          - `ImageService`
    - B:
      - Component: `patina_uefi_event_service`
        - Produces:
          - `EventService`
      - Component: `patina_uefi_protocol_service`
        - Produces:
          - `ProtocolService`
      - Component: `patina_uefi_image_service`
        - Produces:
          - `ImageService`

  At this time, the RFC proposes option A.

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

## Rust Code Design

Todo - The plan right now is to follow this overall layout:

- Component: `patina_uefi_services`
  - Produces:
    - `EventService`
    - `ProtocolService`
    - `ImageService`

And provide API signature for each function in those traits.

## Guide-Level Explanation

Todo
