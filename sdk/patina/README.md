# Patina

The Patina crate provides shared primitives used throughout the Patina project and serves as a "Software Development
Kit (SDK)" for other Patina code.

The crate implements foundational elements used throughout Patina such as the dependency-injected component model,
typed access to interfaces like the UEFI Boot and Runtime Services, Platform Initialization (PI) Specification
content, protocol helpers, logging, performance measurement, and the Patina on-platform testing infrastructure. The
crate builds in `no_std` environments by default, can be paired with either firmware or host tooling, and configured
with optional features.

## Getting started

Add the crate to your manifest and enable the features needed by your firmware or host tooling target.

```toml
[dependencies]
patina = { version = "X.X.X", default-features = false }
```

The crate is `no_std` unless `std` is selected. Tests or host utilities can enable `std` or `mockall` as needed.

## Modules

The SDK is organized into the following root-level modules.

| Module | Description |
| ------ | ----------- |
| **arch** | Abstractions for architecture-specific functionality (e.g. caching) and architecture-specific functions. |
| **base** | Foundational primitives: errors, constants, basic structures. This module gets republished from the root. |
| **component** | Component and service definitions for the dependency-injected component model. |
| **debug** | Macros and definitions for logging and diagnostics. |
| **management_mode** | Definitions for Management Mode (MM/SMM) interactions. |
| **mmio** | Re-export of the `safe-mmio` crate for memory-mapped I/O access. |
| **performance** | Performance measurement types, records, and related GUIDs. |
| **peripheral** | Abstractions and implementations for core device operations. |
| **pi** | Platform Initialization (PI) specification definitions and wrappers. |
| **standard** | Strict definitions of industry standards. |
| **uefi** | UEFI specification definitions and wrappers. |

## Feature Overview

| Feature | Purpose |
| ------- | ------- |
| `alloc` | Allow allocation APIs when targeting `no_std` firmware environments with a custom allocator. |
| `core` | **INTERNAL ONLY** - Expose core internal interfaces. Only for use in the Patina repo. |
| `global_allocator` | Install the global allocator support used by Patina firmware images. |
| `mockall` | Provide mock implementations for Boot Services and other traits (implies `std`). |
| `serde` | Enable serialization support for configuration and PI data structures. |
| `std` | Link the standard library. For example, when building host utilities. |
| `std` | Link the standard library. For example, when building host utilities. |
| `unstable-device-path` | Activate the current device-path parsing and construction prototypes. |
| `unstable` | Opt into experimental APIs gated behind `unstable-*` flags, including device path helpers. |

## Conventions

The SDK uses a set of consistent conventions across the modules.

### Organization

Items are grouped **first by originating specification**, then **by kind**:

```text
patina::
├── base::
│   ├── guid            // Guid, OwnedGuid, BinaryGuid types + Patina-wide identity GUIDs
│   ├── protocol        // ProtocolInterface trait + impls for r-efi protocols
│   ├── error, hash, c_ptr, size/align helpers
│   └── ...
│
├── pi::                // Platform Initialization (PI) Specification
│   ├── guid            // PI GUID constants that do not have a more specific home
│   ├── event           // PI-defined event group GUIDs
│   ├── hob             // HOB types, iterators, and per-HOB payload GUIDs
│   └── protocol        // PI-defined protocol definitions
│
├── uefi::              // UEFI Specification
│   ├── event           // UEFI event types + UEFI-defined event group GUIDs
│   ├── boot_services   // Boot Services trait, allocation, TPL, etc.
│   ├── runtime_services // Runtime service traits
│   ├── device_path, driver_binding, memory_map, decompress, tpl_mutex
│   └── ...
│
├── management_mode::   // MM/SMM (PI Vol. 4 + StandaloneMmPkg)
│   ├── guid            // MM protocol/handle GUIDs
│   ├── event           // MM event group GUIDs
│   └── protocol        // MM protocol definitions
│
├── performance::
│   ├── guid            // FPDT and performance-protocol GUIDs
│   └── config, measurement, record, error
│
└── component::         // Dependency-injection component model
    └── hob             // Hob<T> param and FromHob trait for guided HOB payloads
```

The following conventions are used generally across all modules:

- Root level modules are documented in the [modules](#modules) section of this readme.
- All modules use a `module_name.rs` file in the parent directory instead of a `mod.rs` file in the subdirectory.

### Specification Modules

Modules that are related to a specification, such as `uefi` and `pi`, must only contain definitions and wrappers that
directly relate to definitions from that specification. For example, only UEFI specification protocols should be
defined in `uefi::protocol` and EDKII or other definitions must live elsewhere in their relevant subject module.

### Submodule Names

Module names should be clear and descriptive, and when applicable should not be plural (e.g. `service` not `services`).
Root-level modules are documented in this readme.

Some submodule names have pre-defined purposes and conventions.

| Submodule | Description | Convention |
| --------- | ----------- | ---------- |
| `service` | Definitions for service interfaces for use in the Patina component model | [Services](#services) |
| `protocol` | Definitions and wrappers for UEFI style protocols | [Protocols](#protocols) |
| `hob` | Definitions for HOBs as defined by the PI specification | [HOBs](#hobs) |
| `event` | Definitions for UEFI events and event groups | [Events](#events) |
| `guid` | GUID definitions for use in UEFI | [GUIDs](#guids) |

Other submodules will be specific to their subject, but may be added to conventions if commonality appears.

### Services

Services are defined in their own module under a `service` submodule within a given subject.
(`<module>::service::<service_name>`). Service traits/structs should be descriptive, unique, and be relevant to their
module name. Service definitions should only be colocated into a file if they are directly related to each other.

For general details on services, see the
[Patina Component Model](https://opendevicepartnership.github.io/patina/component/interface.html).

### GUIDs

GUID definitions are only defined once and can be colocated with structure definitions where applicable
(e.g. protocols), and are named with the following conventions.

| Kind | Co-located name | Independent name |
| --- | --- | --- |
| Protocol | `PROTOCOL_GUID` | `..._PROTOCOL_GUID` |
| HOB payload | `HOB_GUID` | `..._HOB_GUID` |
| Event group | - | `..._EVENT_GROUP_GUID` |
| Configuration table | - | `..._TABLE_GUID` |
| Module identity | - | `..._ID` |

If not colocated, or when re-exported, GUID definitions are exposed directly in the `guid` submodule.

### Protocols

Protocols are defined in their own module in the `protocol` module under a given subject/spec module.
(e.g. `pi::protocol::bds::BdsProtocol`)

```rust
// Module Path: subject::protocol::example
use patina::{BinaryGuid, base::protocol::ProtocolInterface};

/// GUID identifying this protocol.
pub const PROTOCOL_GUID: BinaryGuid = BinaryGuid::from_string("16EDC82D-83A3-4E8F-B4D6-294A9FF19218");

/// The C-ABI interface, named after the protocol so callers see `bds::BdsProtocol`, not `bds::Protocol`.
#[repr(C)]
pub struct ExampleProtocol {
    /* function pointers and state */
}

// SAFETY: layout matches the specification.
unsafe impl ProtocolInterface for ExampleProtocol {
    const PROTOCOL_GUID: BinaryGuid = PROTOCOL_GUID;
}
```

Interface naming:

- Descriptive and ends in `Protocol` (`BdsProtocol`, `CpuArchProtocol`, `DecompressProtocol`).
- Supporting types that mirror C/FFI definitions may retain their specification names (e.g.
  `EfiSystemContext`, `EfiExceptionType`).

### HOBs

HOBs are defined, similar to protocols, in their own modules in `module::hob::hob_name`. HOB names must be descriptive,
match their C definitions, and be related to their containing module. PI defined HOBs are only defined in the `pi`
module.

### Events

Event *types* and the UEFI-defined event-group GUIDs live in `uefi::event`:

```rust
use patina::uefi::event::{
    EventType, EventTimerType, EventNotifyCallback,
    EXIT_BOOT_SERVICES_EVENT_GROUP_GUID,
    READY_TO_BOOT_EVENT_GROUP_GUID,
};
```

Event-group GUIDs defined by other specifications live in that specification's `event` module and
follow the `…_EVENT_GROUP_GUID` naming rule:

- `pi::event`: PI-defined groups (e.g. `END_OF_DXE_EVENT_GROUP_GUID`).
- `management_mode::event`: MM-defined groups (e.g. `MM_DISPATCH_EVENT_GROUP_GUID`).

## Additional resources

- [Patina background](https://opendevicepartnership.github.io/patina/patina.html) for project context and design goals.
- [Component getting started guide](https://opendevicepartnership.github.io/patina/component/getting_started.html) for a
    walkthrough that builds on the `component` module.
- The `examples` directory contains host-run samples such as
    [`basic_hob_usage.rs`](https://github.com/OpenDevicePartnership/patina/blob/main/sdk/patina/examples/basic_hob_usage.rs)
    that demonstrate HOB parsing.
