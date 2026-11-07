//! Software Bill of Materials (SBOM) ACPI table definitions.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use super::{ACPI_DESCRIPTION_HEADER_LENGTH, AcpiDescriptionHeader, AcpiParseError};
use core::iter::FusedIterator;
use zerocopy::FromBytes as _;
use zerocopy_derive::{FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned};

/// Signature for the SBOM ACPI table.
pub const EFI_ACPI_SBOM_TABLE_SIGNATURE: u32 = crate::signature!('S', 'B', 'O', 'M');
/// Current revision of the SBOM ACPI table.
pub const EFI_ACPI_SBOM_TABLE_REVISION: u8 = 0x01;
/// No flags are set on the SBOM table entry.
pub const EFI_ACPI_SBOM_TABLE_ENTRY_FLAG_NONE: u8 = 0x00;
/// Reserved SBOM table entry flag.
pub const EFI_ACPI_SBOM_TABLE_ENTRY_FLAG_RESERVED: u8 = 0x01;
/// The payload uses the Concise Software Identification format.
pub const EFI_ACPI_SBOM_TABLE_ENTRY_SBOM_FORMAT_COSWID: u8 = 0x00;
/// The payload uses the `CycloneDX` format.
pub const EFI_ACPI_SBOM_TABLE_ENTRY_SBOM_FORMAT_CYCLONEDX: u8 = 0x01;
/// The payload uses the Software Package Data Exchange format.
pub const EFI_ACPI_SBOM_TABLE_ENTRY_SBOM_FORMAT_SPDX: u8 = 0x02;
/// The payload uses a vendor-defined format.
pub const EFI_ACPI_SBOM_TABLE_ENTRY_SBOM_FORMAT_VENDOR: u8 = 0xFF;
/// The payload is not compressed.
pub const EFI_ACPI_SBOM_TABLE_ENTRY_SBOM_COMPRESSION_NONE: u8 = 0x00;
/// The payload uses zlib compression.
pub const EFI_ACPI_SBOM_TABLE_ENTRY_SBOM_COMPRESSION_ZLIB: u8 = 0x01;
/// The payload uses LZMA compression.
pub const EFI_ACPI_SBOM_TABLE_ENTRY_SBOM_COMPRESSION_LZMA: u8 = 0x02;
/// The payload uses a vendor-defined compression format.
pub const EFI_ACPI_SBOM_TABLE_ENTRY_SBOM_COMPRESSION_VENDOR: u8 = 0xFF;
/// Current revision of an SBOM ACPI table entry.
pub const EFI_ACPI_SBOM_TABLE_ENTRY_REVISION: u8 = 0x04;
/// Length of the fixed SBOM ACPI table entry header.
pub const EFI_ACPI_SBOM_TABLE_ENTRY_HEADER_LENGTH: u16 = 0x000A;

/// SBOM ACPI table (`EFI_ACPI_SBOM_TABLE`).
#[derive(FromBytes, IntoBytes, KnownLayout, Immutable, Unaligned)]
#[repr(C, packed)]
pub struct SbomTable {
    /// Standard ACPI table header.
    pub header: AcpiDescriptionHeader,
    /// Consecutive encoded SBOM table entries, each with its own payload length.
    pub entries: [u8],
}

/// SBOM ACPI table entry (`EFI_ACPI_SBOM_TABLE_ENTRY`).
///
/// Integer fields use the ACPI little-endian wire representation.
#[derive(FromBytes, IntoBytes, KnownLayout, Immutable, Unaligned)]
#[repr(C, packed)]
pub struct SbomTableEntry {
    /// Entry structure revision.
    pub revision: u8,
    /// Length of the fixed entry header.
    pub header_length: u16,
    /// Length of the following payload in bytes.
    pub payload_length: u32,
    /// Entry flags.
    pub flags: u8,
    /// SBOM payload format.
    pub format: u8,
    /// SBOM payload compression format.
    pub compression: u8,
    /// SBOM payload bytes.
    pub payload: [u8],
}

impl SbomTable {
    /// Borrows one table and returns any trailing bytes, without allocating.
    ///
    /// # Errors
    ///
    /// Rejects truncated tables, invalid signatures, unsupported revisions, invalid lengths,
    /// and invalid checksums. Use [`Self::entries`] to validate the entry stream.
    pub fn parse_prefix(bytes: &[u8]) -> Result<(&Self, &[u8]), AcpiParseError> {
        let (header, _) = AcpiDescriptionHeader::ref_from_prefix(bytes).map_err(|_| {
            AcpiParseError::BufferTooSmall { expected: ACPI_DESCRIPTION_HEADER_LENGTH as u64, actual: bytes.len() }
        })?;
        if u32::from_le(header.signature) != EFI_ACPI_SBOM_TABLE_SIGNATURE {
            return Err(AcpiParseError::InvalidField {
                field: stringify!(AcpiDescriptionHeader.signature),
                expected: u64::from(EFI_ACPI_SBOM_TABLE_SIGNATURE),
                found: u64::from(u32::from_le(header.signature)),
            });
        }
        if header.revision != EFI_ACPI_SBOM_TABLE_REVISION {
            return Err(AcpiParseError::InvalidField {
                field: stringify!(AcpiDescriptionHeader.revision),
                expected: u64::from(EFI_ACPI_SBOM_TABLE_REVISION),
                found: u64::from(header.revision),
            });
        }
        let length = u64::from(u32::from_le(header.length));
        if length < ACPI_DESCRIPTION_HEADER_LENGTH as u64 {
            return Err(AcpiParseError::InvalidField {
                field: stringify!(AcpiDescriptionHeader.length),
                expected: ACPI_DESCRIPTION_HEADER_LENGTH as u64,
                found: length,
            });
        }
        let (table, remaining) = split_prefix(bytes, length)?;
        let checksum_sum = table.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte));
        if checksum_sum != 0 {
            return Err(AcpiParseError::InvalidField {
                field: stringify!(AcpiDescriptionHeader.checksum),
                expected: u64::from(header.checksum.wrapping_sub(checksum_sum)),
                found: u64::from(header.checksum),
            });
        }
        let table = Self::ref_from_bytes(table).map_err(|_| AcpiParseError::InvalidLayout)?;
        Ok((table, remaining))
    }

    /// Iterates over borrowed entries without allocating.
    ///
    /// Each entry's revision, header length, and payload length are checked. The iterator
    /// yields an error once and then stops if an entry is malformed. Exhaust the iterator
    /// to validate the entire stream. Payload format and compression identifiers are
    /// preserved without interpreting the payload.
    pub fn entries(&self) -> SbomTableEntries<'_> {
        SbomTableEntries { remaining: &self.entries }
    }
}

impl SbomTableEntry {
    /// Borrows one entry and returns the remaining stream, without allocating.
    ///
    /// # Errors
    ///
    /// Rejects unsupported revisions, invalid fixed header lengths, truncated payloads,
    /// and lengths that overflow the address space.
    pub fn parse_prefix(bytes: &[u8]) -> Result<(&Self, &[u8]), AcpiParseError> {
        let header_length = usize::from(EFI_ACPI_SBOM_TABLE_ENTRY_HEADER_LENGTH);
        let (header, _) = split_prefix(bytes, header_length as u64)?;
        let header = Self::ref_from_bytes(header).map_err(|_| AcpiParseError::InvalidLayout)?;
        if header.revision != EFI_ACPI_SBOM_TABLE_ENTRY_REVISION {
            return Err(AcpiParseError::InvalidField {
                field: stringify!(SbomTableEntry.revision),
                expected: u64::from(EFI_ACPI_SBOM_TABLE_ENTRY_REVISION),
                found: u64::from(header.revision),
            });
        }
        if u16::from_le(header.header_length) != EFI_ACPI_SBOM_TABLE_ENTRY_HEADER_LENGTH {
            return Err(AcpiParseError::InvalidField {
                field: stringify!(SbomTableEntry.header_length),
                expected: u64::from(EFI_ACPI_SBOM_TABLE_ENTRY_HEADER_LENGTH),
                found: u64::from(u16::from_le(header.header_length)),
            });
        }
        let payload_length = u64::from(u32::from_le(header.payload_length));
        let length = header_length as u64 + payload_length;
        let (entry, remaining) = split_prefix(bytes, length)?;
        let entry = Self::ref_from_bytes(entry).map_err(|_| AcpiParseError::InvalidLayout)?;
        Ok((entry, remaining))
    }
}

/// An allocation-free, fallible iterator over SBOM table entries.
pub struct SbomTableEntries<'a> {
    remaining: &'a [u8],
}

impl<'a> Iterator for SbomTableEntries<'a> {
    type Item = Result<&'a SbomTableEntry, AcpiParseError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining.is_empty() {
            return None;
        }
        match SbomTableEntry::parse_prefix(self.remaining) {
            Ok((entry, remaining)) => {
                self.remaining = remaining;
                Some(Ok(entry))
            }
            Err(error) => {
                self.remaining = &[];
                Some(Err(error))
            }
        }
    }
}

impl FusedIterator for SbomTableEntries<'_> {}

fn split_prefix(bytes: &[u8], length: u64) -> Result<(&[u8], &[u8]), AcpiParseError> {
    let error = AcpiParseError::BufferTooSmall { expected: length, actual: bytes.len() };
    let length = usize::try_from(length).map_err(|_| error)?;
    bytes.split_at_checked(length).ok_or(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerocopy::{FromBytes, IntoBytes};

    fn valid_table() -> [u8; 59] {
        let header = AcpiDescriptionHeader {
            signature: EFI_ACPI_SBOM_TABLE_SIGNATURE.to_le(),
            length: 59u32.to_le(),
            revision: EFI_ACPI_SBOM_TABLE_REVISION,
            ..Default::default()
        };
        let mut bytes = [0; 59];
        bytes[..36].copy_from_slice(header.as_bytes());
        bytes[36..]
            .copy_from_slice(&[4, 10, 0, 3, 0, 0, 0, 0, 2, 0, b'a', b'b', b'c', 4, 10, 0, 0, 0, 0, 0, 0, 0xFF, 0xFF]);
        fix_checksum(&mut bytes);
        bytes
    }

    fn fix_checksum(bytes: &mut [u8]) {
        bytes[9] = 0;
        bytes[9] = 0u8.wrapping_sub(bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)));
    }

    #[test]
    fn test_sbom_iteration_borrows_variable_length_and_empty_payloads() {
        let bytes = valid_table();
        let (table, rest) = SbomTable::parse_prefix(&bytes).unwrap();
        assert!(rest.is_empty());
        let mut entries = table.entries();
        let first = entries.next().unwrap().unwrap();
        assert_eq!(&first.payload, b"abc");
        assert_eq!(first.as_bytes().as_ptr(), bytes[36..].as_ptr());
        let second = entries.next().unwrap().unwrap();
        assert!(second.payload.is_empty());
        assert_eq!(second.format, 0xFF);
        assert!(entries.next().is_none());
        assert!(entries.next().is_none());
    }

    #[test]
    fn test_sbom_iteration_reports_later_error_and_stops() {
        let mut bytes = valid_table();
        bytes[49] = 3;
        fix_checksum(&mut bytes);
        let (table, _) = SbomTable::parse_prefix(&bytes).unwrap();
        let mut entries = table.entries();
        assert!(entries.next().unwrap().is_ok());
        assert_eq!(
            entries.next().unwrap().err(),
            Some(AcpiParseError::InvalidField { field: "SbomTableEntry.revision", expected: 4, found: 3 })
        );
        assert!(entries.next().is_none());
        assert!(entries.next().is_none());
    }

    #[test]
    fn test_sbom_entry_rejects_bad_lengths_and_truncation() {
        let bytes = valid_table();
        for length in 0..10 {
            assert_eq!(
                SbomTableEntry::parse_prefix(&bytes[36..36 + length]).err(),
                Some(AcpiParseError::BufferTooSmall { expected: 10, actual: length })
            );
        }
        assert_eq!(
            SbomTableEntry::parse_prefix(&bytes[36..48]).err(),
            Some(AcpiParseError::BufferTooSmall { expected: 13, actual: 12 })
        );
        let mut entry = [4, 9, 0, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(
            SbomTableEntry::parse_prefix(&entry).err(),
            Some(AcpiParseError::InvalidField { field: "SbomTableEntry.header_length", expected: 10, found: 9 })
        );
        entry[1] = 10;
        entry[3..7].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(
            SbomTableEntry::parse_prefix(&entry).err(),
            Some(AcpiParseError::BufferTooSmall { expected: u64::from(u32::MAX) + 10, actual: 10 })
        );
    }

    #[test]
    fn test_sbom_table_prefix_remainder_and_empty_stream() {
        let bytes = valid_table();
        let mut buffer = [0; 61];
        buffer[..59].copy_from_slice(&bytes);
        buffer[59..].copy_from_slice(&[0xAA, 0xBB]);
        let (table, remaining) = SbomTable::parse_prefix(&buffer).unwrap();
        assert_eq!(table.as_bytes().len(), 59);
        assert_eq!(remaining, &[0xAA, 0xBB]);
        let mut empty = [0; 36];
        empty.copy_from_slice(&bytes[..36]);
        empty[4..8].copy_from_slice(&36u32.to_le_bytes());
        fix_checksum(&mut empty);
        assert!(SbomTable::parse_prefix(&empty).unwrap().0.entries().next().is_none());
        let expected_checksum = buffer[9];
        buffer[9] ^= 1;
        assert_eq!(
            SbomTable::parse_prefix(&buffer).err(),
            Some(AcpiParseError::InvalidField {
                field: "AcpiDescriptionHeader.checksum",
                expected: u64::from(expected_checksum),
                found: u64::from(buffer[9]),
            })
        );
    }

    #[test]
    fn test_sbom_entry_layout() {
        let bytes = [0u8; 10];
        let entry = SbomTableEntry::ref_from_bytes(&bytes).unwrap();
        assert_eq!(core::mem::size_of_val(entry), usize::from(EFI_ACPI_SBOM_TABLE_ENTRY_HEADER_LENGTH));
        assert_eq!(core::mem::align_of_val(entry), 1);
        assert_eq!(core::mem::offset_of!(SbomTableEntry, revision), 0);
        assert_eq!(core::mem::offset_of!(SbomTableEntry, header_length), 1);
        assert_eq!(core::mem::offset_of!(SbomTableEntry, payload_length), 3);
        assert_eq!(core::mem::offset_of!(SbomTableEntry, flags), 7);
        assert_eq!(core::mem::offset_of!(SbomTableEntry, format), 8);
        assert_eq!(core::mem::offset_of!(SbomTableEntry, compression), 9);
        assert!(entry.payload.is_empty());
        assert_eq!(EFI_ACPI_SBOM_TABLE_SIGNATURE.to_le_bytes(), *b"SBOM");
    }

    #[test]
    fn test_sbom_variable_length_layouts() {
        let header = AcpiDescriptionHeader {
            signature: EFI_ACPI_SBOM_TABLE_SIGNATURE,
            length: 49,
            revision: EFI_ACPI_SBOM_TABLE_REVISION,
            ..Default::default()
        };
        let entry_header = [
            EFI_ACPI_SBOM_TABLE_ENTRY_REVISION,
            10,
            0,
            3,
            0,
            0,
            0,
            EFI_ACPI_SBOM_TABLE_ENTRY_FLAG_NONE,
            EFI_ACPI_SBOM_TABLE_ENTRY_SBOM_FORMAT_SPDX,
            EFI_ACPI_SBOM_TABLE_ENTRY_SBOM_COMPRESSION_NONE,
        ];
        let mut bytes = [0u8; 50];
        bytes[1..37].copy_from_slice(header.as_bytes());
        bytes[37..47].copy_from_slice(&entry_header);
        bytes[47..].copy_from_slice(b"abc");
        let table = SbomTable::ref_from_bytes(&bytes[1..]).unwrap();
        let entry = SbomTableEntry::ref_from_bytes(&table.entries).unwrap();
        assert_eq!(core::mem::size_of_val(table), 49);
        assert_eq!(&entry.as_bytes()[..10], &entry_header);
        let header_length = entry.header_length;
        let payload_length = entry.payload_length;
        assert_eq!(header_length, EFI_ACPI_SBOM_TABLE_ENTRY_HEADER_LENGTH);
        assert_eq!(payload_length, 3);
        assert_eq!(entry.revision, EFI_ACPI_SBOM_TABLE_ENTRY_REVISION);
        assert_eq!(entry.format, EFI_ACPI_SBOM_TABLE_ENTRY_SBOM_FORMAT_SPDX);
        assert_eq!(&entry.payload, b"abc");
        assert!(SbomTable::ref_from_bytes(&bytes[..35]).is_err());
        assert!(SbomTableEntry::ref_from_bytes(&bytes[..9]).is_err());
        assert!(SbomTable::ref_from_bytes(header.as_bytes()).unwrap().entries.is_empty());
        assert!(SbomTableEntry::ref_from_bytes(&entry_header).unwrap().payload.is_empty());
    }
}
