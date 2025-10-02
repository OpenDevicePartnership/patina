//! Error types and conversions for the Firmware File System (FFS) crate.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use patina::error::EfiError;
use r_efi::efi;

/// Error definitions for Firmware File System
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirmwareFileSystemError {
    /// The FFS header is invalid or malformed.
    InvalidHeader,
    /// The block map structure is invalid.
    InvalidBlockMap,
    /// A parameter provided to a function is invalid.
    InvalidParameter,
    /// The requested operation or feature is unsupported.
    Unsupported,
    /// The FFS is in an invalid or unexpected state.
    InvalidState,
    /// Data corruption was detected in the FFS.
    DataCorrupt,
    /// The FFS structure has not been composed yet.
    NotComposed,
    /// The FFS structure has not been extracted yet.
    NotExtracted,
    /// The FFS node is not a leaf node.
    NotLeaf,
    /// Composing the FFS structure failed.
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
