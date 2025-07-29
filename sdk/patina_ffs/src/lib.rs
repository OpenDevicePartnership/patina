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

use patina_sdk::error::EfiError;
use r_efi::efi;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirmwareFileSystemError {
    InvalidHeader,
    InvalidBlockMap,
    InvalidParameter,
    Unsupported,
    InvalidState,
    DataCorrupt,
    NotComposed,
    NotExtracted,
    NotLeaf,
    ComposeFailed,
}

impl From<FirmwareFileSystemError> for EfiError {
    fn from(value: FirmwareFileSystemError) -> Self {
        match value {
            FirmwareFileSystemError::InvalidParameter
            | FirmwareFileSystemError::NotComposed
            | FirmwareFileSystemError::NotExtracted
            | FirmwareFileSystemError::NotLeaf => EfiError::InvalidParameter,
            FirmwareFileSystemError::Unsupported => EfiError::Unsupported,
            FirmwareFileSystemError::InvalidHeader
            | FirmwareFileSystemError::InvalidBlockMap
            | FirmwareFileSystemError::InvalidState
            | FirmwareFileSystemError::DataCorrupt => EfiError::VolumeCorrupted,
            FirmwareFileSystemError::ComposeFailed => EfiError::DeviceError,
        }
    }
}

impl From<FirmwareFileSystemError> for efi::Status {
    fn from(value: FirmwareFileSystemError) -> Self {
        let err: EfiError = value.into();
        err.into()
    }
}
