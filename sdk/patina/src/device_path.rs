//! Device Path Definitions
//!
//! Defines UEFI device path node structures and utilities for walking and parsing paths and nodes.
//! Uses the UEFI specification as a reference for struct definitions and parsing logic:
//! <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html>
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

/// Module for FV-related Device Path struct implementations.
pub mod fv_types;
/// Module for spec-defined device path node types defined in this module.
#[cfg(feature = "unstable-device-path")]
pub mod node_defs;
/// Module for defining device path nodes and methods for creating and parsing them.
#[cfg(feature = "unstable-device-path")]
pub mod parse_node;
/// Module for UEFI Device Path Utilities, providing various utilities for interacting with and parsing UEFI device paths.
#[cfg(feature = "unstable-device-path")]
pub mod paths;
/// Module for walking UEFI device paths, providing utilities for traversing and analyzing device path structures.
pub mod walker;
