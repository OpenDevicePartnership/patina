//! Platform Initialization (PI) Specification Definitions and Support Code
//!
//! This crate provides constants, definitions, and support code for the Platform Initialization (PI) Specification
//! maintained by the UEFI Forum. The primary focus is to provide rust types and constants to build a PI Specification
//! compliant firmware, functionality is allowed to be implemented around those types as well. Code implementation
//! must be applicable to any PI spec compliant firmware that may need the associated functionality.
//!
//! Refer to the Platform Initialization (PI) Specification for more background on the specification.
//! <https://uefi.org/specs/PI/1.8A/>
//!
//! # UEFI Environment
//!
//! This crate depends on the r-efi crate for UEFI defined constants and definitions defined in the UEFI Specification.
//! That project contains important details about writing Rust code agains the UEFI Specification. Those details are
//! not repeated here. Read that project's documentation when setting up a UEFI project.
//!
//! # Current State
//!
//! The PI Specification describes a number of boot phases and concepts used across those boot phases referred to as
//! "Shared Architectural Elements". The implementation predominantly focuses on the DXE phase at this time. This
//! includes shared elements used in other stages such as the HOBs and firmware storage. Contributions to expand
//! coverage of the specification are welcome.
//!
//! The overall structure and design of the crate is subject to breaking changes at this time. The code is being
//! shared for wider feedback and collaboration that might result in fundamental changes to the organization of
//! content. These details will be documented and managed using appropriately versioned releases of the crate to
//! reflect the degree of change.
//!

#![cfg_attr(not(test), no_std)]
#![allow(missing_docs)]

extern crate alloc;

mod address_helper;
mod boot_mode;

pub mod dxe_services;
pub mod fw_fs;
pub mod hob;
pub mod list_entry;
pub mod protocols;
#[cfg(feature = "serde")]
pub mod serializable;
pub mod status_code;

pub use boot_mode::Mode as BootMode;
