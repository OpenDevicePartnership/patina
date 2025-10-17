//! SMBIOS service interfaces
//!
//! This module defines the public service types for SMBIOS operations.
//! These are the primary interfaces that platform code uses to interact with SMBIOS.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

extern crate alloc;
use alloc::vec::Vec;
use core::cell::Ref;
use r_efi::efi::Handle;
use zerocopy_derive::{FromBytes, Immutable, IntoBytes as DeriveIntoBytes, KnownLayout};

/// SMBIOS record handle type (16-bit identifier)
pub type SmbiosHandle = u16;

/// SMBIOS record type
pub type SmbiosType = u8;

/// Special handle value for automatic assignment
pub const SMBIOS_HANDLE_PI_RESERVED: SmbiosHandle = 0xFFFE;

/// SMBIOS string maximum length per specification
pub const SMBIOS_STRING_MAX_LENGTH: usize = 64;

/// SMBIOS table header structure
///
/// This is the standard 4-byte header that appears at the start of every SMBIOS record.
/// It contains the record type, length of structured data, and a unique handle.
#[repr(C, packed)]
#[derive(Debug, Clone, PartialEq, FromBytes, DeriveIntoBytes, Immutable, KnownLayout)]
pub struct SmbiosTableHeader {
    /// SMBIOS record type
    pub record_type: SmbiosType,
    /// Length of the structured data (including header)
    pub length: u8,
    /// Unique handle for this record
    pub handle: SmbiosHandle,
}

impl SmbiosTableHeader {
    /// Creates a new SMBIOS table header
    pub fn new(record_type: SmbiosType, length: u8, handle: SmbiosHandle) -> Self {
        Self { record_type, length, handle }
    }
}

/// Iterator over SMBIOS records
///
/// This iterator is used internally by the SMBIOS manager for:
/// - C protocol `GetNext` implementation (EDKII compatibility)
/// - Internal iteration during table publication
/// - Test validation
///
/// **Note:** This iterator is not exposed through the public `Service<Smbios>` API.
/// Platform code typically adds records using `add_record<T>()` and then publishes
/// the table for the OS to query directly.
///
/// # Type Filtering
///
/// The iterator can optionally filter by record type. If `None` is provided,
/// all records are returned. If `Some(type)` is provided, only records of
/// that type are returned.
pub struct SmbiosRecordsIter<'a> {
    records: Ref<'a, Vec<crate::manager::SmbiosRecord>>,
    position: usize,
    filter_type: Option<SmbiosType>,
}

impl<'a> SmbiosRecordsIter<'a> {
    /// Create a new iterator over SMBIOS records
    pub(crate) fn new(records: Ref<'a, Vec<crate::manager::SmbiosRecord>>, filter_type: Option<SmbiosType>) -> Self {
        Self { records, position: 0, filter_type }
    }
}

impl<'a> Iterator for SmbiosRecordsIter<'a> {
    type Item = (SmbiosTableHeader, Option<Handle>);

    fn next(&mut self) -> Option<Self::Item> {
        while self.position < self.records.len() {
            let record = &self.records[self.position];
            self.position += 1;

            // Apply type filter if specified
            if let Some(filter) = self.filter_type
                && record.header.record_type != filter
            {
                continue;
            }

            return Some((record.header.clone(), record.producer_handle));
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    use std::format;

    #[test]
    fn test_smbios_table_header_new() {
        let header = SmbiosTableHeader::new(0, 24, 0x0001);
        assert_eq!(header.record_type, 0);
        assert_eq!(header.length, 24);
        // Use local variable to avoid packed field alignment issues
        let handle = header.handle;
        assert_eq!(handle, 0x0001);
    }

    #[test]
    fn test_smbios_table_header_clone() {
        let header1 = SmbiosTableHeader::new(1, 32, 0x0002);
        let header2 = header1.clone();
        assert_eq!(header1, header2);
    }

    #[test]
    fn test_smbios_table_header_debug() {
        let header = SmbiosTableHeader::new(127, 4, 0xFFFF);
        let debug_str = format!("{:?}", header);
        assert!(debug_str.contains("127"));
        assert!(debug_str.contains("4"));
    }

    #[test]
    fn test_smbios_handle_pi_reserved() {
        assert_eq!(SMBIOS_HANDLE_PI_RESERVED, 0xFFFE);
    }

    #[test]
    fn test_smbios_string_max_length() {
        assert_eq!(SMBIOS_STRING_MAX_LENGTH, 64);
    }
}
