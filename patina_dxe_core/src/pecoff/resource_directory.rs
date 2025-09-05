//! UEFI PE/COFF Resource Directory Support
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use core::mem;
use scroll::Pread;

/// Type that represents a header for the UEFI image resource directory.
#[derive(PartialEq, Debug, Pread)]
#[repr(C)]
pub struct Directory {
    /// The characteristics of the resource directory.
    pub characteristics: u32,
    /// The time stamp of the resource directory.
    pub time_date_stamp: u32,
    /// The major version of the resource directory.
    pub major_version: u16,
    /// The minor version of the resource directory.
    pub minor_version: u16,
    /// The number of named entries in the resource directory.
    pub number_of_named_entries: u16,
    /// The number of ID entries in the resource directory.
    pub number_of_id_entries: u16,
    // Array of EfiImageResourceDirectoryEntry entries follows.
}

impl Directory {
    pub fn total_entries(&self) -> usize {
        (self.number_of_named_entries + self.number_of_id_entries) as usize
    }

    pub fn size_in_bytes(&self) -> usize {
        mem::size_of::<Self>() + self.total_entries() * mem::size_of::<DirectoryEntry>()
    }
}

/// Type that represents a string in the UEFI image resource directory.
#[derive(PartialEq, Debug, Pread)]
#[repr(C)]
pub struct DirectoryString {
    /// The length of the string in characters.
    pub length: u16,
    // A UTF-16 string follows.
}

/// Type that represents a data entry in the UEFI image resource directory.
#[derive(PartialEq, Debug, Pread)]
#[repr(C)]
pub struct DataEntry {
    /// The offset to the data from the beginning of the resource directory.
    pub offset_to_data: u32,
    /// The size of the data in bytes.
    pub size: u32,
    /// The code page of the data.
    pub code_page: u32,
    /// Reserved.
    pub reserved: u32,
}

/// Type that represents an entry in the UEFI image resource directory.
#[derive(PartialEq, Debug, Pread)]
#[repr(C)]
pub struct DirectoryEntry {
    /// The ID of the entry.
    pub id: u32,
    /// The offset to the data from the beginning of the resource directory.
    pub data: u32,
}

impl DirectoryEntry {
    pub fn name_offset(&self) -> u32 {
        self.id & 0x7fffffff
    }
    pub fn name_is_string(&self) -> bool {
        (self.id & 0x80000000) != 0
    }
    pub fn offset_to_directory(&self) -> u32 {
        self.data & 0x7fffffff
    }
    pub fn data_is_directory(&self) -> bool {
        (self.data & 0x80000000) != 0
    }
}
