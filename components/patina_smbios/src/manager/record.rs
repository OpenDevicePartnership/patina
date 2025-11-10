//! Internal SMBIOS record structures and builder
//!
//! This module provides internal record representation and serialization utilities
//! for SMBIOS table construction.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

extern crate alloc;

use alloc::vec::Vec;

use crate::service::SmbiosTableHeader;
use r_efi::efi::Handle;

/// Internal SMBIOS record representation
///
/// This implementation is for SMBIOS 3.0+ specification which uses 64-bit addressing.
pub struct SmbiosRecord {
    /// SMBIOS table header
    pub header: SmbiosTableHeader,
    /// Optional handle of the producer that created this record
    pub producer_handle: Option<Handle>,
    /// Complete record data including header and strings
    pub data: Vec<u8>,
    pub(super) string_count: usize,
}

impl SmbiosRecord {
    /// Creates a new SMBIOS record
    pub fn new(header: SmbiosTableHeader, producer_handle: Option<Handle>, data: Vec<u8>, string_count: usize) -> Self {
        Self { header, producer_handle, data, string_count }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    use std::vec;

    use r_efi::efi;

    #[test]
    fn test_smbios_record_new() {
        let header = SmbiosTableHeader::new(1, 10, 5);
        let data = vec![1u8, 10, 5, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let record = SmbiosRecord::new(header, None, data.clone(), 0);
        let record_type = record.header.record_type;
        let handle = record.header.handle;
        assert_eq!(record_type, 1);
        assert_eq!(handle, 5);
        assert_eq!(record.producer_handle, None);
        assert_eq!(record.data, data);
        assert_eq!(record.string_count, 0);
    }

    #[test]
    fn test_smbios_record_with_producer_handle() {
        let header = SmbiosTableHeader::new(2, 8, 10);
        let producer = core::ptr::null_mut::<()>() as efi::Handle;
        let data = vec![2u8, 8, 10, 0, 0, 0, 0, 0];
        let record = SmbiosRecord::new(header, Some(producer), data, 2);
        assert_eq!(record.string_count, 2);
        assert_eq!(record.producer_handle, Some(producer));
    }

    #[test]
    fn test_smbios_record_with_string_count() {
        let header = SmbiosTableHeader::new(1, 4, 10);
        let data = vec![1, 4, 10, 0, 0, 0];
        let record = SmbiosRecord::new(header, None, data, 5);
        assert_eq!(record.string_count, 5);
    }
}
