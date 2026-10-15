//! UEFI PE/COFF Resource Directory Support
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
// cspell:ignore elems
//!
use core::mem::size_of;

use zerocopy::{FromBytes as _, LittleEndian, U16, U32};
use zerocopy_derive::{FromBytes, Immutable, KnownLayout};

use super::error;

const RESOURCE_SECTION: &str = "resource section";

/// A UEFI image resource directory followed by its directory entries.
#[derive(FromBytes, Immutable, KnownLayout)]
#[repr(C)]
pub struct Directory {
    /// The characteristics of the resource directory.
    characteristics: U32<LittleEndian>,
    /// The time stamp of the resource directory.
    time_date_stamp: U32<LittleEndian>,
    /// The major version of the resource directory.
    major_version: U16<LittleEndian>,
    /// The minor version of the resource directory.
    minor_version: U16<LittleEndian>,
    /// The number of named entries in the resource directory.
    number_of_named_entries: U16<LittleEndian>,
    /// The number of ID entries in the resource directory.
    number_of_id_entries: U16<LittleEndian>,
    /// The named entries followed by the ID entries.
    entries: [DirectoryEntry],
}

impl Directory {
    const HEADER_SIZE: usize = 16;

    /// Parses a resource directory and all entries declared by its header.
    pub fn parse(bytes: &[u8]) -> error::Result<&Self> {
        let (header, _) = Self::ref_from_prefix_with_elems(bytes, 0)
            .map_err(|_| error::Error::BufferTooShort(Self::HEADER_SIZE, RESOURCE_SECTION))?;
        let entry_count = usize::from(header.number_of_named_entries.get())
            .saturating_add(usize::from(header.number_of_id_entries.get()));
        let entries_size = entry_count
            .checked_mul(size_of::<DirectoryEntry>())
            .ok_or(error::Error::BufferTooShort(usize::MAX, RESOURCE_SECTION))?;
        let required = Self::HEADER_SIZE
            .checked_add(entries_size)
            .ok_or(error::Error::BufferTooShort(usize::MAX, RESOURCE_SECTION))?;

        Self::ref_from_prefix_with_elems(bytes, entry_count)
            .map(|(directory, _)| directory)
            .map_err(|_| error::Error::BufferTooShort(required, RESOURCE_SECTION))
    }

    /// Returns the named entries at the beginning of the directory entry array.
    pub fn named_entries(&self) -> &[DirectoryEntry] {
        self.entries
            .get(..usize::from(self.number_of_named_entries.get()))
            .expect("the parsed entry array contains every declared named entry")
    }

    /// Returns the first named or ID entry.
    pub fn first_entry(&self) -> Option<&DirectoryEntry> {
        self.entries.first()
    }
}

/// A UTF-16 string in the UEFI image resource directory.
#[derive(FromBytes, Immutable, KnownLayout)]
#[repr(C)]
pub struct DirectoryString {
    /// The length of the string in UTF-16 code units.
    length: U16<LittleEndian>,
    /// The UTF-16LE code units.
    string: [U16<LittleEndian>],
}

impl DirectoryString {
    const HEADER_SIZE: usize = 2;

    /// Parses a resource directory string.
    pub fn parse(bytes: &[u8]) -> error::Result<&Self> {
        let (header, _) = Self::ref_from_prefix_with_elems(bytes, 0)
            .map_err(|_| error::Error::BufferTooShort(Self::HEADER_SIZE, RESOURCE_SECTION))?;
        let length = usize::from(header.length.get());
        let string_size = length
            .checked_mul(size_of::<U16<LittleEndian>>())
            .ok_or(error::Error::BufferTooShort(usize::MAX, RESOURCE_SECTION))?;
        let required = Self::HEADER_SIZE
            .checked_add(string_size)
            .ok_or(error::Error::BufferTooShort(usize::MAX, RESOURCE_SECTION))?;

        Self::ref_from_prefix_with_elems(bytes, length)
            .map(|(string, _)| string)
            .map_err(|_| error::Error::BufferTooShort(required, RESOURCE_SECTION))
    }

    /// Returns whether this string contains the provided UTF-16 code units.
    pub fn matches(&self, expected: &[u16]) -> bool {
        self.string.iter().map(|unit| unit.get()).eq(expected.iter().copied())
    }
}

/// A data entry in the UEFI image resource directory.
#[derive(Debug, FromBytes, Immutable, KnownLayout)]
#[repr(C)]
pub struct DataEntry {
    /// The offset to the data from the beginning of the resource directory.
    offset_to_data: U32<LittleEndian>,
    /// The size of the data in bytes.
    size: U32<LittleEndian>,
    /// The code page used to decode code-point values within the resource data.
    code_page: U32<LittleEndian>,
    /// Reserved.
    reserved: U32<LittleEndian>,
}

impl DataEntry {
    /// Parses a resource data entry.
    pub fn parse(bytes: &[u8]) -> error::Result<&Self> {
        Self::ref_from_prefix(bytes)
            .map(|(entry, _)| entry)
            .map_err(|_| error::Error::BufferTooShort(size_of::<Self>(), RESOURCE_SECTION))
    }

    /// Returns the offset to the resource data.
    pub fn offset_to_data(&self) -> u32 {
        self.offset_to_data.get()
    }

    /// Returns the size of the resource data.
    pub fn size(&self) -> u32 {
        self.size.get()
    }
}

/// An entry in the UEFI image resource directory.
#[derive(Debug, FromBytes, Immutable, KnownLayout)]
#[repr(C)]
pub struct DirectoryEntry {
    /// The integer ID or high-bit-tagged offset to a resource directory string.
    id: U32<LittleEndian>,
    /// The offset to a data entry or high-bit-tagged offset to another resource directory.
    data: U32<LittleEndian>,
}

impl DirectoryEntry {
    const NAME_OFFSET_MASK: u32 = 0x7fff_ffff;
    const NAME_IS_STRING_MASK: u32 = 0x8000_0000;
    const DIRECTORY_OFFSET_MASK: u32 = 0x7fff_ffff;
    const DATA_IS_DIRECTORY_MASK: u32 = 0x8000_0000;

    /// Returns the offset to the resource directory string containing this entry's name.
    pub fn name_offset(&self) -> u32 {
        self.id.get() & Self::NAME_OFFSET_MASK
    }

    /// Returns whether the entry is named by a resource directory string.
    pub fn name_is_string(&self) -> bool {
        (self.id.get() & Self::NAME_IS_STRING_MASK) != 0
    }

    /// Returns the offset to the resource directory referenced by this entry.
    pub fn offset_to_directory(&self) -> u32 {
        self.data.get() & Self::DIRECTORY_OFFSET_MASK
    }

    /// Returns the offset to the resource data entry referenced by this entry.
    pub fn data_offset(&self) -> u32 {
        self.data.get()
    }

    /// Returns whether the entry references another resource directory.
    pub fn data_is_directory(&self) -> bool {
        (self.data.get() & Self::DATA_IS_DIRECTORY_MASK) != 0
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn test_directory_borrows_declared_entries() {
        let mut bytes = [0u8; Directory::HEADER_SIZE + size_of::<DirectoryEntry>()];
        bytes[12..14].copy_from_slice(&1u16.to_le_bytes());
        bytes[16..20].copy_from_slice(&0x8000_1234u32.to_le_bytes());
        bytes[20..24].copy_from_slice(&0x8000_5678u32.to_le_bytes());

        let directory = Directory::parse(&bytes).unwrap();
        let entry = &directory.named_entries()[0];
        assert!(entry.name_is_string());
        assert_eq!(entry.name_offset(), 0x1234);
        assert!(entry.data_is_directory());
        assert_eq!(entry.offset_to_directory(), 0x5678);
    }

    #[test]
    fn test_directory_rejects_overclaimed_entries() {
        let mut bytes = [0u8; Directory::HEADER_SIZE + size_of::<DirectoryEntry>()];
        bytes[12..14].copy_from_slice(&2u16.to_le_bytes());

        assert!(matches!(Directory::parse(&bytes), Err(error::Error::BufferTooShort(..))));
    }

    #[test]
    fn test_directory_string_borrows_utf16le_units() {
        let bytes = [3, 0, b'H', 0, b'I', 0, b'I', 0];
        let string = DirectoryString::parse(&bytes).unwrap();
        assert!(string.matches(&[u16::from(b'H'), u16::from(b'I'), u16::from(b'I')]));
    }
}
