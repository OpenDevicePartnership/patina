//! Software Bill of Materials (SBOM) discovery protocol definition.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use crate::uefi::acpi_table::{AcpiParseError, sbom::SbomTableEntry};
use crate::{BinaryGuid, protocol::ProtocolInterface};

/// GUID identifying the SBOM discovery protocol.
pub const PROTOCOL_GUID: BinaryGuid = BinaryGuid::from_string("6f84698b-c4e9-4bd0-9c8f-5eb6c6f46871");

/// Protocol descriptor referencing one serialized SBOM table entry.
///
/// It is intended to be installed on the same handle as an image's Loaded Image Protocol
/// when SBOM data is detected for that image.
///
/// Providers keep the descriptor and referenced bytes immutable and readable while installed.
#[repr(C)]
pub struct SbomProtocol {
    /// Pointer to the serialized entry's first byte.
    pub entry: *const u8,
    /// Complete entry length in bytes, including the header and payload.
    pub entry_size: usize,
}

impl SbomProtocol {
    /// Borrows and structurally validates the referenced entry.
    ///
    /// # Safety
    ///
    /// A non-null `entry` must point to `entry_size` initialized bytes in one allocation,
    /// readable and immutable for the lifetime of the returned borrow.
    ///
    /// # Errors
    ///
    /// Returns an error for a null pointer, an unrepresentable slice length, malformed entry,
    /// or an entry whose embedded lengths differ from `entry_size`.
    pub unsafe fn entry(&self) -> Result<&SbomTableEntry, AcpiParseError> {
        if self.entry.is_null() || self.entry_size > isize::MAX as usize {
            return Err(AcpiParseError::InvalidLayout);
        }
        // SAFETY: The caller guarantees the allocation and its lifetime; the pointer and
        // slice length have been checked above. The borrow is tied to this descriptor.
        let bytes = unsafe { core::slice::from_raw_parts(self.entry, self.entry_size) };
        let (entry, remaining) = SbomTableEntry::parse_prefix(bytes)?;
        if !remaining.is_empty() {
            return Err(AcpiParseError::InvalidField {
                field: "SbomProtocol.entry_size",
                expected: (self.entry_size - remaining.len()) as u64,
                found: self.entry_size as u64,
            });
        }
        Ok(entry)
    }
}

// SAFETY: This is the EFI_SBOM_ENTRY_PROTOCOL C descriptor layout for this GUID.
unsafe impl ProtocolInterface for SbomProtocol {
    const PROTOCOL_GUID: BinaryGuid = PROTOCOL_GUID;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sbom_protocol_guid() {
        assert_eq!(
            <SbomProtocol as ProtocolInterface>::PROTOCOL_GUID,
            BinaryGuid::from_string("6f84698b-c4e9-4bd0-9c8f-5eb6c6f46871")
        );
        assert_eq!(core::mem::size_of::<SbomProtocol>(), 2 * core::mem::size_of::<usize>());
        assert_eq!(core::mem::offset_of!(SbomProtocol, entry), 0);
        assert_eq!(core::mem::offset_of!(SbomProtocol, entry_size), core::mem::size_of::<usize>());
    }

    #[test]
    fn test_sbom_protocol_entry_validation() {
        let bytes = [4, 10, 0, 3, 0, 0, 0, 0, 2, 0, b'a', b'b', b'c', 0];
        let mut protocol = SbomProtocol { entry: bytes.as_ptr(), entry_size: 13 };
        // SAFETY: All tested lengths fit within bytes, which remains immutable and alive.
        unsafe {
            assert_eq!(&protocol.entry().unwrap().payload, b"abc");
            protocol.entry_size = 12;
            assert!(matches!(protocol.entry(), Err(AcpiParseError::BufferTooSmall { .. })));
            protocol.entry_size = 14;
            assert!(matches!(protocol.entry(), Err(AcpiParseError::InvalidField { .. })));
            protocol.entry_size = 0;
            assert!(matches!(protocol.entry(), Err(AcpiParseError::BufferTooSmall { .. })));
        }
        protocol.entry = core::ptr::null();
        // SAFETY: Null pointers are rejected before access.
        assert!(matches!(unsafe { protocol.entry() }, Err(AcpiParseError::InvalidLayout)));
        protocol.entry = bytes.as_ptr();
        protocol.entry_size = usize::MAX;
        // SAFETY: Unrepresentable lengths are rejected before access.
        assert!(matches!(unsafe { protocol.entry() }, Err(AcpiParseError::InvalidLayout)));
    }
}
