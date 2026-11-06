//! Software Bill of Materials (SBOM) guided HOB definitions.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use alloc::boxed::Box;
use patina::{
    BinaryGuid,
    component::hob::{FromHob, HobParseError},
    uefi::acpi_table::{AcpiParseError, sbom::SbomTableEntry},
};
use zerocopy::{FromBytes, IntoBytes};

/// GUID of a guided HOB whose payload contains one [`SbomTableEntry`].
///
/// There is no ACPI description header. The entry's header and payload lengths exclude
/// any trailing HOB alignment padding.
pub const SBOM_HOB_GUID: BinaryGuid = BinaryGuid::from_string("16797303-315f-41d3-bf33-3df925756f05");

/// An owned SBOM entry parsed from a guided HOB identified by [`SBOM_HOB_GUID`].
///
/// Each HOB contains one entry, not a complete ACPI table. Multiple HOBs can be consumed
/// through [`Hob<SbomHob>`](patina::component::hob::Hob).
///
/// Parsing validates the entry revision and lengths without interpreting its payload.
/// Trailing padding to the next eight-byte HOB boundary is accepted but not retained.
#[derive(Debug)]
pub struct SbomHob {
    bytes: Box<[u8]>,
}

impl SbomHob {
    /// Returns the entry stored in this HOB, excluding HOB alignment padding.
    pub fn entry(&self) -> &SbomTableEntry {
        SbomTableEntry::ref_from_bytes(&self.bytes).expect("SBOM HOB bytes retain their validated entry layout")
    }
}

impl FromHob for SbomHob {
    const HOB_GUID: BinaryGuid = SBOM_HOB_GUID;

    fn parse(bytes: &[u8]) -> Result<Self, HobParseError> {
        let (entry, padding) = SbomTableEntry::parse_prefix(bytes).map_err(hob_parse_error)?;
        let padding_length = (8 - entry.as_bytes().len() % 8) % 8;
        if !padding.is_empty() && padding.len() != padding_length {
            return Err(HobParseError::Invalid("unexpected bytes after the SBOM entry"));
        }

        Ok(Self { bytes: entry.as_bytes().into() })
    }
}

fn hob_parse_error(error: AcpiParseError) -> HobParseError {
    match error {
        AcpiParseError::BufferTooSmall { expected, actual } => HobParseError::BufferTooSmall {
            // The HOB error cannot represent a required length larger than usize.
            expected: usize::try_from(expected).unwrap_or(usize::MAX),
            actual,
        },
        AcpiParseError::InvalidField { field, .. } => HobParseError::Invalid(field),
        AcpiParseError::InvalidLayout => HobParseError::Invalid("invalid ACPI structure layout"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;
    use patina::component::Storage;

    fn entry_bytes() -> Vec<u8> {
        alloc::vec![4, 10, 0, 3, 0, 0, 0, 0, 2, 0, b'a', b'b', b'c']
    }

    #[test]
    fn test_sbom_hob_owns_one_entry_and_discards_padding() {
        let expected = entry_bytes();
        let mut bytes = expected.clone();
        bytes.extend_from_slice(&[0xFF; 3]);
        let hob = SbomHob::parse(&bytes).unwrap();
        bytes.fill(0);
        assert_eq!(hob.entry().as_bytes(), &expected);
        assert_eq!(&hob.entry().payload, b"abc");
        assert_eq!(SbomHob::HOB_GUID, SBOM_HOB_GUID);
        assert_eq!(SBOM_HOB_GUID, BinaryGuid::from_string("16797303-315f-41d3-bf33-3df925756f05"));
        assert_eq!(SbomHob::parse(&expected).unwrap().entry().as_bytes(), &expected);
    }

    #[test]
    fn test_sbom_hob_rejects_truncated_header_and_payload() {
        let bytes = entry_bytes();
        for length in 0..10 {
            assert_eq!(
                SbomHob::parse(&bytes[..length]).unwrap_err(),
                HobParseError::BufferTooSmall { expected: 10, actual: length }
            );
        }
        assert_eq!(
            SbomHob::parse(&bytes[..12]).unwrap_err(),
            HobParseError::BufferTooSmall { expected: 13, actual: 12 }
        );
    }

    #[test]
    fn test_sbom_hob_rejects_invalid_entry_fields() {
        for (offset, value, field) in [(0, 3, "SbomTableEntry.revision"), (1, 9, "SbomTableEntry.header_length")] {
            let mut bytes = entry_bytes();
            bytes[offset] = value;
            assert_eq!(SbomHob::parse(&bytes).unwrap_err(), HobParseError::Invalid(field));
        }
    }

    #[test]
    fn test_sbom_hob_maps_acpi_parse_errors() {
        assert_eq!(
            hob_parse_error(AcpiParseError::InvalidLayout),
            HobParseError::Invalid("invalid ACPI structure layout")
        );
        assert_eq!(
            hob_parse_error(AcpiParseError::BufferTooSmall { expected: u64::MAX, actual: 10 }),
            HobParseError::BufferTooSmall { expected: usize::MAX, actual: 10 }
        );
        assert_eq!(
            hob_parse_error(AcpiParseError::BufferTooSmall { expected: 13, actual: 12 }),
            HobParseError::BufferTooSmall { expected: 13, actual: 12 }
        );
    }

    #[test]
    fn test_sbom_hob_rejects_extra_entries_and_invalid_padding_lengths() {
        let entry = entry_bytes();
        for count in [1, 2, 4] {
            let mut bytes = entry.clone();
            bytes.resize(bytes.len() + count, 0);
            assert_eq!(
                SbomHob::parse(&bytes).unwrap_err(),
                HobParseError::Invalid("unexpected bytes after the SBOM entry")
            );
        }
        let mut bytes = entry.clone();
        bytes.extend_from_slice(&entry);
        assert!(SbomHob::parse(&bytes).is_err());
    }

    #[test]
    fn test_sbom_hob_registers_multiple_independent_entries() {
        let bytes = entry_bytes();
        let mut storage = Storage::new();
        SbomHob::register(&bytes, &mut storage);
        SbomHob::register(&bytes, &mut storage);
        let hob = storage.get_hob::<SbomHob>().unwrap();
        assert_eq!(hob.iter().count(), 2);
        for entry in &hob {
            assert_eq!(entry.entry().as_bytes(), &bytes);
        }
    }

    #[test]
    fn test_sbom_hob_accepts_entry_without_payload() {
        let bytes = [4, 10, 0, 0, 0, 0, 0, 0, 2, 0];
        let hob = SbomHob::parse(&bytes).unwrap();
        assert!(hob.entry().payload.is_empty());
    }

    #[test]
    fn test_sbom_hob_rejects_acpi_table_header() {
        let mut bytes = [0u8; 36];
        bytes[..4].copy_from_slice(b"SBOM");
        assert!(SbomHob::parse(&bytes).is_err());
    }
}
