# RFC: `Move CPU functionality into Patina Core`

// TODO: SHERRY make copilot write this

## Change Log

- 2026-01-27: Initial RFC created.

## Motivation

The current Patina implementation of the Device Path protocol is disorganized. Issues include:
- Definitions of device path types across multiple crates (`fv.rs`, `measurement.rs`, etc.)
- Parsing functionality split across multiple crates (e.g. `patina_internal_device_path`, `uefi_protocol`)
- The usage of crates labeled `internal` by external crates (e.g. `patina_performance` relies on `patina_internal_device_path`)

This RFC presents a strategy for integrating all Device-Path-related functionality into a single crate as part of the Patina SDK.

## Technology Background

For basic information on the Device Path Protocol, see the [UEFI Spec Vol 10](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html). This RFC focuses more on its organization within Patina than specifics of Deivce Path functionality.

The Patina SDK provides shared primitives used in the rest of the Patina core. For more information, see the [README](/sdk/patina/README.md). As many core implementations across several crates depend on Device Path functionality, `sdk` is the best consolidated location to place these struct definitions and parsing functions.

## Goals

1. Consolidate all Device Path functionality into the Patina SDK crate
2. Clearly delineate internal vs. external functionality within the SDK


## Requirements

1. Organize all Device Path struct definitions into a `device_path` module inside `sdk`
2. Organize all Device Path parsing functions into a `device_path` module inside `sdk`
3. Preserve internal `core` Device Path functionality, ensuring it is not used outside `core`
4. Move externally used Device Path structs and functions into the `device_path` module into `sdk`

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

- device_path: types.rs
- device_paths: parse.rs
- additional stuff???
- is this tuff in Patina 💀💀☠️☠️☠️

## Guide-Level Explanation

N/A
