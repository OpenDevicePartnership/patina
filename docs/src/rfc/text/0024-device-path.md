# RFC: Consolidate and Reorganize Device Path Functionality

## Summary

This RFC proposes consolidating all Device Path Protocol functionality, currently scattered across multiple Patina
crates, into a single `device_path` module within the Patina SDK. The reorganization addresses issues with disorganized
struct definitions, fragmented parsing functionality, and improper usage of internal crates by external components.
This consolidated interface also allows improved node construction patterns.

## Change Log

- 2026-01-27: Initial RFC created.
- 2026-02-03: Minor wording changes based on reviews.
- 2026-02-04: Moving to FCP.

## Motivation

The current Patina implementation of the Device Path protocol is disorganized. Issues include:

- Definitions of device path types across multiple crates (`fv.rs`, `measurement.rs`, etc.)
- Parsing functionality split across multiple crates (e.g. `patina_internal_device_path`, `uefi_protocol`)
- The usage of crates labeled `internal` by external crates (e.g. `patina_performance` relies on `patina_internal_device_path`)

This RFC presents a strategy for integrating all Device-Path-related functionality into a single crate as part of the
Patina SDK.

## Technology Background

For basic information on the Device Path Protocol, see the
[UEFI Spec Vol 10](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html). This RFC focuses more on
its organization within Patina than specifics of Device Path functionality.

The Patina SDK provides shared primitives used in the rest of the Patina core and in external callers. For more
information, see the [README](/sdk/patina/README.md). As many core implementations across several crates depend on
Device Path functionality, `sdk` is the best consolidated location to place these struct definitions and parsing functions.

## Goals

1. Consolidate all Device Path functionality into the Patina SDK crate
2. Clearly delineate internal vs. external functionality within the SDK

## Requirements

1. Organize all Device Path struct definitions into a `device_path` module inside `sdk`
2. Organize all Device Path parsing functions into a `device_path` module inside `sdk`
   1. *If necessary, preserve internal `core` Device Path functionality, ensuring it is not used outside `core`*
3. Move externally used Device Path structs and functions into the `device_path` module into `sdk`

## Unresolved Questions

1. Is the Patina SDK the best location for Device Path functionality?

## Prior Art

Currently, Device Path functionality and structs are split across multiple crates.

The following crates contain Device Path struct definitions:

- `patina_dxe_core`: `fv.rs`
- `sdk/patina_performance`: `measurement.rs`
- `patina_internal_device_path`

The following crates contain Device Path parsing functionality:

- `patina_internal_device_path`
- `sdk/uefi_protocol`
- `patina_dxe_core/image.rs`

The following crates incorrectly use `patina_internal_device_path`, which should be internal to `core`:

- `patina_performance`

## Alternatives

1. Keep as is.
2. Put Device Path functionality somewhere other than `sdk`, such as its own crate.

## Rust Code Design

All device path functionality will be moved to the Patina SDK.

```text
└── sdk/patina
    ├── src
        ├── arch
        ├── ...
        ├── device_path
            ├── node.rs
            ├── traversal.rs
```

`node.rs` includes all Device Path struct variations, such as `MediaFwVolDevicePath`. `traversal.rs` includes all Device
Path traversal functionality, such as `DevicePathWalker`.

The crates that use `patina_internal_device_path` will have to use `patina` as a dependency instead. This includes
`patina_performance` and `patina_dxe_core`. This should not present any issues as both these crates already reference
other functionality inside `patina`.

### Node Construction Improvements

By putting functionality into the Patina SDK, we are also able to control usage and initialization of Device Path
structs. A major improvement to the current scheme of initializing raw fields in Device Path nodes would be to fix
internal fields and only allow crates to construct nodes through `pub` functions.

For a certain node type, the header should be fixed. As such, internally in the SDK, we can set these fields in the
constructor to their fixed values.

```rust
/* NEW IMPLEMENTATION */
#[repr(C)]
pub struct MediaFwVolDevicePath {
    header: efi::protocols::device_path::Protocol,
    name: efi::Guid,
}

impl MediaFwVolDevicePath {
    pub fn new(name: efi::Guid) -> Self {
        Self {
            header: efi::protocols::device_path::Protocol {
                r#type: efi::protocols::device_path::TYPE_MEDIA, // fixed type
                sub_type: efi::protocols::device_path::Media::SUBTYPE_PIWG_FIRMWARE_VOLUME, // fixed subtype
                length: std::mem::size_of::<Self>() as u16, // fixed size
            },
            name,
        }
    }
}

MediaFwVolDevicePath::new(efi::Guid(...));

/* CURRENT IMPLEMENTATION 
MediaFwVolDevicePath {
    header: efi::protocols::device_path::Protocol {
        r#type: efi::protocols::device_path::TYPE_MEDIA,
        sub_type,
        length: [
            (mem::size_of::<MediaFwVolDevicePath>() & 0xff) as u8,
            ((mem::size_of::<MediaFwVolDevicePath>() >> 8) & 0xff) as u8,
        ],
    },
    name,
}
*/
```
