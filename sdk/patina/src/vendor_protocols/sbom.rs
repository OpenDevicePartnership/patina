//! Software Bill of Materials (SBOM) discovery protocol definition.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use crate::uefi::acpi_table::sbom::SbomTableEntry;
use crate::{BinaryGuid, protocol::ProtocolInterface};

/// GUID identifying the SBOM discovery protocol.
pub const PROTOCOL_GUID: BinaryGuid = BinaryGuid::from_string("6f84698b-c4e9-4bd0-9c8f-5eb6c6f46871");

/// Software Bill of Materials (SBOM) discovery protocol.
///
/// Exposes an SBOM entry. The protocol interface points directly to a serialized SBOM
/// table entry consisting of a fixed header followed by its payload. The total entry
/// length is the sum of the header length and payload length fields.
///
/// Providers must keep the complete entry readable and immutable while the protocol is
/// installed.
pub type SbomProtocol = SbomTableEntry;

// SAFETY: The protocol GUID identifies the packed SBOM entry wire layout.
unsafe impl ProtocolInterface for SbomProtocol {
    const PROTOCOL_GUID: BinaryGuid = PROTOCOL_GUID;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uefi::acpi_table::AcpiParseError;

    #[test]
    fn test_sbom_protocol_guid() {
        assert_eq!(
            <SbomProtocol as ProtocolInterface>::PROTOCOL_GUID,
            BinaryGuid::from_string("6f84698b-c4e9-4bd0-9c8f-5eb6c6f46871")
        );
    }

    #[test]
    fn test_sbom_protocol_entry_validation() {
        let mut bytes = [4, 10, 0, 3, 0, 0, 0, 0, 2, 0, b'a', b'b', b'c'];
        // SAFETY: The complete declared entry is readable and immutable during this borrow.
        assert_eq!(&unsafe { SbomProtocol::from_raw(bytes.as_ptr()) }.unwrap().payload, b"abc");
        bytes[3] = 0;
        // SAFETY: The header declares an empty payload, within the allocation.
        assert!(unsafe { SbomProtocol::from_raw(bytes.as_ptr()) }.unwrap().payload.is_empty());
        bytes[0] = 3;
        // SAFETY: The complete fixed header is readable; its invalid revision is rejected.
        assert!(matches!(unsafe { SbomProtocol::from_raw(bytes.as_ptr()) }, Err(AcpiParseError::InvalidField { .. })));
        bytes[0] = 4;
        bytes[1] = 9;
        // SAFETY: The complete fixed header is readable; its invalid length is rejected.
        assert!(matches!(unsafe { SbomProtocol::from_raw(bytes.as_ptr()) }, Err(AcpiParseError::InvalidField { .. })));
        // SAFETY: Null pointers are rejected without access.
        assert!(matches!(unsafe { SbomProtocol::from_raw(core::ptr::null()) }, Err(AcpiParseError::InvalidLayout)));
    }
}
