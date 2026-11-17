//! UEFI ACPI Table Protocol and ACPI table definitions.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use crate::{BinaryGuid, error::EfiError, protocol::ProtocolInterface, standard::efi};
use core::{ffi::c_void, fmt};
use zerocopy_derive::{FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned};

#[cfg(feature = "unstable-sbom")]
pub mod sbom;

/// UEFI `EFI_ACPI_TABLE_PROTOCOL` interface.
///
/// Installs and uninstalls ACPI tables in the platform's ACPI root tables.
/// See the [UEFI ACPI Table Protocol specification](https://uefi.org/specs/UEFI/2.10/20_Protocols_ACPI_Protocols.html).
#[repr(C)]
pub struct AcpiTableProtocol {
    /// Installs a copy of an ACPI table and returns its key through the final parameter.
    ///
    /// # Safety
    ///
    /// The interface pointer must identify a live instance of this protocol. The buffer must
    /// be readable for the supplied byte length, and the key pointer must
    /// be valid for writing a `usize`. All pointers must remain valid throughout the call.
    /// The caller retains ownership of the buffer.
    install: unsafe extern "efiapi" fn(*const Self, *const c_void, usize, *mut usize) -> efi::Status,
    /// Uninstalls the ACPI table identified by a key returned from `install`.
    ///
    /// # Safety
    ///
    /// The interface pointer must identify a live instance of this protocol for the duration
    /// of the call.
    uninstall: unsafe extern "efiapi" fn(*const Self, usize) -> efi::Status,
}

impl AcpiTableProtocol {
    /// Installs a copy of the serialized ACPI table and returns its table key.
    ///
    /// The caller retains ownership of the bytes. Table validation and checksum handling
    /// are performed by the protocol implementation.
    ///
    /// # Errors
    ///
    /// Returns the protocol's EFI error, including invalid table parameters, unsupported
    /// tables, or insufficient resources.
    pub fn install(&self, table: &[u8]) -> Result<usize, EfiError> {
        let mut key = 0;
        // SAFETY: self is a live protocol interface. The slice is readable for table.len()
        // bytes and key is writable throughout the call. The protocol validates the table
        // and copies its bytes rather than retaining the caller's buffer.
        let status = unsafe { (self.install)(self, table.as_ptr().cast(), table.len(), &raw mut key) };
        if status.is_error() { Err(EfiError::from(status)) } else { Ok(key) }
    }

    /// Uninstalls a table using the key returned by [`Self::install`].
    ///
    /// # Errors
    ///
    /// Returns the protocol's EFI error, including [`EfiError::NotFound`] when the key
    /// does not identify an installed table.
    pub fn uninstall(&self, key: usize) -> Result<(), EfiError> {
        // SAFETY: self is a live protocol interface. The protocol validates the table key;
        // no caller-provided memory is accessed.
        let status = unsafe { (self.uninstall)(self, key) };
        if status.is_error() { Err(EfiError::from(status)) } else { Ok(()) }
    }
}

// SAFETY: This is the two-function EFI_ACPI_TABLE_PROTOCOL C layout for this UEFI GUID.
unsafe impl ProtocolInterface for AcpiTableProtocol {
    const PROTOCOL_GUID: BinaryGuid = BinaryGuid::from_string("ffe06bdd-6107-46a6-7bb2-5a9c7ec5275c");
}

/// Length in bytes of a standard ACPI description header.
pub const ACPI_DESCRIPTION_HEADER_LENGTH: usize = core::mem::size_of::<AcpiDescriptionHeader>();

/// Byte offset of the checksum in a standard ACPI description header.
pub const ACPI_CHECKSUM_OFFSET: usize = core::mem::offset_of!(AcpiDescriptionHeader, checksum);

/// An error parsing an ACPI table or a structure within a table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpiParseError {
    /// The buffer is shorter than the declared structure.
    BufferTooSmall {
        /// Required number of bytes, including lengths larger than the address space.
        expected: u64,
        /// Available number of bytes.
        actual: usize,
    },
    /// A field does not satisfy its required value.
    InvalidField {
        /// Qualified field name.
        field: &'static str,
        /// Required value, or minimum value for a length field.
        expected: u64,
        /// Value found in the structure.
        found: u64,
    },
    /// A structure cannot be represented by its wire layout.
    InvalidLayout,
}

impl fmt::Display for AcpiParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BufferTooSmall { expected, actual } => {
                write!(f, "Buffer too small: expected {expected} bytes, got {actual}")
            }
            Self::InvalidField { field, expected, found } => {
                write!(f, "Invalid ACPI field {field}: expected {expected:#x}, found {found:#x}")
            }
            Self::InvalidLayout => f.write_str("Invalid ACPI structure layout"),
        }
    }
}

impl core::error::Error for AcpiParseError {}

/// Standard ACPI description header (`EFI_ACPI_DESCRIPTION_HEADER`).
///
/// Integer fields use the ACPI little-endian wire representation. The checksum is chosen so
/// that the wrapping sum of all bytes in the complete table is zero.
#[derive(Debug, Default, Clone, Copy, FromBytes, IntoBytes, KnownLayout, Immutable, Unaligned)]
#[repr(C, packed)]
pub struct AcpiDescriptionHeader {
    /// Four-byte table signature.
    pub signature: u32,
    /// Total table length, including this header.
    pub length: u32,
    /// Table revision.
    pub revision: u8,
    /// Checksum of the complete table.
    pub checksum: u8,
    /// Original equipment manufacturer identifier.
    pub oem_id: [u8; 6],
    /// Original equipment manufacturer table identifier.
    pub oem_table_id: [u8; 8],
    /// Original equipment manufacturer revision.
    pub oem_revision: u32,
    /// Vendor identifier of the utility that created the table.
    pub creator_id: u32,
    /// Revision of the utility that created the table.
    pub creator_revision: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerocopy::{FromBytes, IntoBytes};

    unsafe extern "efiapi" fn test_install(
        protocol: *const AcpiTableProtocol,
        bytes: *const c_void,
        length: usize,
        key: *mut usize,
    ) -> efi::Status {
        assert!(!protocol.is_null());
        if length == 0 {
            return efi::Status::INVALID_PARAMETER;
        }
        // SAFETY: The wrapper supplies readable slice bytes and a writable key for the call.
        let table = unsafe { core::slice::from_raw_parts(bytes.cast::<u8>(), length) };
        assert_eq!(table, b"serialized table");
        // SAFETY: The wrapper supplies a writable key for the duration of the call.
        unsafe { *key = 42 };
        efi::Status::SUCCESS
    }

    unsafe extern "efiapi" fn test_uninstall(protocol: *const AcpiTableProtocol, key: usize) -> efi::Status {
        assert!(!protocol.is_null());
        if key == 42 { efi::Status::SUCCESS } else { efi::Status::NOT_FOUND }
    }

    #[test]
    fn test_acpi_table_protocol_wrappers_forward_bytes_keys_and_errors() {
        let protocol = AcpiTableProtocol { install: test_install, uninstall: test_uninstall };
        assert_eq!(protocol.install(b"serialized table"), Ok(42));
        assert_eq!(protocol.install(&[]), Err(EfiError::InvalidParameter));
        assert_eq!(protocol.uninstall(42), Ok(()));
        assert_eq!(protocol.uninstall(43), Err(EfiError::NotFound));
    }

    #[test]
    fn test_acpi_table_protocol_layout_and_guid() {
        use core::mem::{align_of, offset_of, size_of};
        assert_eq!(size_of::<AcpiTableProtocol>(), 2 * size_of::<usize>());
        assert_eq!(align_of::<AcpiTableProtocol>(), align_of::<usize>());
        assert_eq!(offset_of!(AcpiTableProtocol, install), 0);
        assert_eq!(offset_of!(AcpiTableProtocol, uninstall), size_of::<usize>());
        assert_eq!(AcpiTableProtocol::PROTOCOL_GUID, BinaryGuid::from_string("ffe06bdd-6107-46a6-7bb2-5a9c7ec5275c"));
    }

    #[test]
    fn test_acpi_parse_error_display() {
        use core::fmt::Write;

        let error = AcpiParseError::InvalidField { field: "SbomTableEntry.revision", expected: 4, found: 3 };
        let mut message = alloc::string::String::new();
        write!(&mut message, "{error}").unwrap();
        assert_eq!(message, "Invalid ACPI field SbomTableEntry.revision: expected 0x4, found 0x3");
    }

    #[test]
    fn test_acpi_description_header_layout() {
        assert_eq!(ACPI_DESCRIPTION_HEADER_LENGTH, 36);
        assert_eq!(ACPI_CHECKSUM_OFFSET, 9);
        assert_eq!(core::mem::align_of::<AcpiDescriptionHeader>(), 1);
        assert_eq!(core::mem::offset_of!(AcpiDescriptionHeader, length), 4);
        assert_eq!(core::mem::offset_of!(AcpiDescriptionHeader, oem_id), 10);
        assert_eq!(core::mem::offset_of!(AcpiDescriptionHeader, oem_table_id), 16);
        assert_eq!(core::mem::offset_of!(AcpiDescriptionHeader, oem_revision), 24);
        assert_eq!(core::mem::offset_of!(AcpiDescriptionHeader, creator_id), 28);
        assert_eq!(core::mem::offset_of!(AcpiDescriptionHeader, creator_revision), 32);
    }

    #[test]
    fn test_acpi_description_header_unaligned_round_trip() {
        let header = AcpiDescriptionHeader {
            signature: u32::from_le_bytes(*b"SBOM"),
            length: 36,
            revision: 1,
            checksum: 0x42,
            oem_id: *b"PATINA",
            oem_table_id: *b"SBOMTEST",
            oem_revision: 2,
            creator_id: 3,
            creator_revision: 4,
        };
        let mut bytes = [0u8; 37];
        bytes[1..].copy_from_slice(header.as_bytes());
        let parsed = AcpiDescriptionHeader::ref_from_bytes(&bytes[1..]).unwrap();
        assert_eq!(parsed.as_bytes(), header.as_bytes());
        assert!(AcpiDescriptionHeader::ref_from_bytes(&bytes[1..36]).is_err());
    }
}
