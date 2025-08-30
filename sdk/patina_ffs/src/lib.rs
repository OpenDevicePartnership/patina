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
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod file;
pub mod section;
pub mod volume;

pub enum FirmwareFileSystemError {
    InvalidHeader,
    InvalidState,
    DataCorrupt,
    NotComposed,
}
