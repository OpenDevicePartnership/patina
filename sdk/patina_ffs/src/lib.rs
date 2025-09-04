//! Support for Firmware File System as described in the UEFI Platform
//! Initialization Specification.
//!
//! This crate implements support for accesssing and generating Firmware File
//! System (FFS) structures.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod err;
pub mod file;
pub mod section;
pub mod volume;

pub use err::FirmwareFileSystemError;
