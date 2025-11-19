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
use zerocopy_derive::*;

#[cfg(any(test, feature = "mockall"))]
use mockall::automock;

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
#[derive(Debug, Clone, PartialEq, FromBytes, IntoBytes, Immutable, KnownLayout)]
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

/// Object-safe trait for SMBIOS service operations
///
/// This trait defines the core SMBIOS operations that can be used through
/// a trait object (`Service<dyn Smbios>`), enabling mocking and testing.
///
/// Generic operations like `add_record<T>()` are provided via an extension
/// implementation on `Service<dyn Smbios>`.
#[cfg_attr(any(test, feature = "mockall"), automock)]
pub trait Smbios {
    /// Gets the SMBIOS version information.
    ///
    /// # Returns
    ///
    /// A tuple of (major_version, minor_version).
    fn version(&self) -> (u8, u8);

    /// Publishes the SMBIOS table to the UEFI Configuration Table
    ///
    /// # Returns
    ///
    /// Returns a tuple of (table_address, entry_point_address) on success.
    ///
    /// # Errors
    ///
    /// Returns `SmbiosError` if no records, allocation fails, or installation fails.
    fn publish_table(
        &self,
    ) -> core::result::Result<(r_efi::efi::PhysicalAddress, r_efi::efi::PhysicalAddress), crate::error::SmbiosError>;

    /// Updates a string in an existing SMBIOS record.
    ///
    /// # Arguments
    ///
    /// * `smbios_handle` - Handle of the record to update
    /// * `string_number` - 1-based index of the string to update
    /// * `string` - New string value
    fn update_string(
        &self,
        smbios_handle: SmbiosHandle,
        string_number: usize,
        string: &str,
    ) -> core::result::Result<(), crate::error::SmbiosError>;

    /// Removes an SMBIOS record from the SMBIOS table.
    ///
    /// # Arguments
    ///
    /// * `smbios_handle` - Handle of the record to remove
    fn remove(&self, smbios_handle: SmbiosHandle) -> core::result::Result<(), crate::error::SmbiosError>;

    /// Add an SMBIOS record from raw bytes.
    ///
    /// This is the non-generic version used internally by `add_record<T>()`.
    ///
    /// # Arguments
    ///
    /// * `producer_handle` - Optional handle of the producer creating this record
    /// * `bytes` - Serialized SMBIOS record bytes
    fn add_from_bytes(
        &self,
        producer_handle: Option<r_efi::efi::Handle>,
        bytes: &[u8],
    ) -> core::result::Result<SmbiosHandle, crate::error::SmbiosError>;
}

/// SMBIOS service implementation
///
/// This struct implements `Smbios` and is registered as `Service<dyn Smbios>`.
/// Generic operations are available via extension methods on the service.
///
/// # Example
///
/// ```ignore
/// fn entry_point(
///     smbios: Service<dyn Smbios>,
/// ) -> Result<()> {
///     // Add structured records using generic extension method
///     smbios.add_record(None, &bios_info)?;
///
///     // Publish to configuration table
///     smbios.publish_table()?;
///     Ok(())
/// }
/// ```
#[derive(patina::component::service::IntoService)]
#[service(dyn Smbios)]
pub struct SmbiosImpl {
    pub(crate) manager: patina::tpl_mutex::TplMutex<
        'static,
        crate::manager::SmbiosManager,
        patina::boot_services::StandardBootServices,
    >,
    pub(crate) boot_services: &'static patina::boot_services::StandardBootServices,
    pub(crate) major_version: u8,
    pub(crate) minor_version: u8,
}

impl SmbiosImpl {
    /// Get a reference to the manager for protocol installation
    ///
    /// This is only used during component initialization to install the C/EDKII protocol
    pub(crate) fn manager(
        &self,
    ) -> &patina::tpl_mutex::TplMutex<'static, crate::manager::SmbiosManager, patina::boot_services::StandardBootServices>
    {
        &self.manager
    }
}

// Methods below are tested via integration (Q35 platform component)
// They require StandardBootServices and TplMutex which aren't mockable in unit tests
#[cfg_attr(coverage_nightly, coverage(off))]
impl Smbios for SmbiosImpl {
    fn version(&self) -> (u8, u8) {
        (self.major_version, self.minor_version)
    }

    fn publish_table(
        &self,
    ) -> core::result::Result<(r_efi::efi::PhysicalAddress, r_efi::efi::PhysicalAddress), crate::error::SmbiosError>
    {
        let manager = self.manager.lock();
        manager.publish_table(self.boot_services)
    }

    fn update_string(
        &self,
        smbios_handle: SmbiosHandle,
        string_number: usize,
        string: &str,
    ) -> core::result::Result<(), crate::error::SmbiosError> {
        let manager = self.manager.lock();
        manager.update_string(smbios_handle, string_number, string)
    }

    fn remove(&self, smbios_handle: SmbiosHandle) -> core::result::Result<(), crate::error::SmbiosError> {
        let manager = self.manager.lock();
        manager.remove(smbios_handle)
    }

    fn add_from_bytes(
        &self,
        producer_handle: Option<r_efi::efi::Handle>,
        bytes: &[u8],
    ) -> core::result::Result<SmbiosHandle, crate::error::SmbiosError> {
        let manager = self.manager.lock();
        manager.add_from_bytes(producer_handle, bytes)
    }
}

/// Extension trait providing generic methods for SMBIOS service
///
/// This trait provides the type-safe `add_record<T>()` method as an extension
/// on `Service<dyn Smbios>`. The generic method can't be part of the
/// trait object itself (trait objects can't have generic methods), so we
/// implement it as an extension trait.
///
/// # Usage
///
/// ```ignore
/// use patina_smbios::service::SmbiosExt;
///
/// fn my_component(smbios: Service<dyn Smbios>) {
///     let bios_info = Type0PlatformFirmwareInformation { ... };
///     let handle = smbios.add_record(None, &bios_info)?;
/// }
/// ```
pub trait SmbiosExt {
    /// Add an SMBIOS record from a structured type.
    ///
    /// This is a type-safe convenience method that automatically serializes
    /// a structured record and adds it to the SMBIOS table.
    ///
    /// # Arguments
    ///
    /// * `producer_handle` - Optional handle of the producer creating this record
    /// * `record` - A reference to any type implementing `SmbiosRecordStructure`
    ///
    /// # Returns
    ///
    /// Returns the assigned SMBIOS handle for the newly added record.
    fn add_record<T>(
        &self,
        producer_handle: Option<r_efi::efi::Handle>,
        record: &T,
    ) -> core::result::Result<SmbiosHandle, crate::error::SmbiosError>
    where
        T: crate::smbios_record::SmbiosRecordStructure;
}

/// Implementation of extension trait for `Service<dyn Smbios>`
#[cfg_attr(coverage_nightly, coverage(off))]
impl SmbiosExt for patina::component::service::Service<dyn Smbios> {
    fn add_record<T>(
        &self,
        producer_handle: Option<r_efi::efi::Handle>,
        record: &T,
    ) -> core::result::Result<SmbiosHandle, crate::error::SmbiosError>
    where
        T: crate::smbios_record::SmbiosRecordStructure,
    {
        let bytes = record.to_bytes();
        self.add_from_bytes(producer_handle, &bytes)
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

    #[test]
    fn test_smbios_table_header_equality() {
        let header1 = SmbiosTableHeader::new(0, 24, 0x0001);
        let header2 = SmbiosTableHeader::new(0, 24, 0x0001);
        let header3 = SmbiosTableHeader::new(1, 24, 0x0001);

        assert_eq!(header1, header2);
        assert_ne!(header1, header3);
    }

    #[test]
    fn test_smbios_table_header_from_bytes() {
        use zerocopy::IntoBytes;
        let header = SmbiosTableHeader::new(127, 4, 0xFFFE);
        let bytes = header.as_bytes();
        assert_eq!(bytes.len(), 4);
        assert_eq!(bytes[0], 127); // type
        assert_eq!(bytes[1], 4); // length
    }

    #[test]
    fn test_smbios_records_iter_basic() {
        use crate::manager::SmbiosRecord;
        use alloc::vec;
        use core::cell::RefCell;

        let records = RefCell::new(vec![
            SmbiosRecord::new(SmbiosTableHeader::new(0, 24, 0x0001), None, vec![], 0),
            SmbiosRecord::new(SmbiosTableHeader::new(1, 32, 0x0002), None, vec![], 0),
        ]);

        let borrowed = records.borrow();
        let mut iter = SmbiosRecordsIter::new(borrowed, None);

        let first = iter.next().unwrap();
        assert_eq!(first.0.record_type, 0);
        let handle = first.0.handle;
        assert_eq!(handle, 0x0001);

        let second = iter.next().unwrap();
        assert_eq!(second.0.record_type, 1);
        let handle = second.0.handle;
        assert_eq!(handle, 0x0002);

        assert!(iter.next().is_none());
    }

    #[test]
    fn test_smbios_records_iter_with_filter() {
        use crate::manager::SmbiosRecord;
        use alloc::vec;
        use core::cell::RefCell;

        let records = RefCell::new(vec![
            SmbiosRecord::new(SmbiosTableHeader::new(0, 24, 0x0001), None, vec![], 0),
            SmbiosRecord::new(SmbiosTableHeader::new(1, 32, 0x0002), None, vec![], 0),
            SmbiosRecord::new(SmbiosTableHeader::new(0, 24, 0x0003), None, vec![], 0),
        ]);

        let borrowed = records.borrow();
        let mut iter = SmbiosRecordsIter::new(borrowed, Some(0));

        let first = iter.next().unwrap();
        assert_eq!(first.0.record_type, 0);
        let handle = first.0.handle;
        assert_eq!(handle, 0x0001);

        let second = iter.next().unwrap();
        assert_eq!(second.0.record_type, 0);
        let handle = second.0.handle;
        assert_eq!(handle, 0x0003);

        assert!(iter.next().is_none());
    }

    #[test]
    fn test_smbios_records_iter_empty() {
        use crate::manager::SmbiosRecord;
        use alloc::vec;
        use core::cell::RefCell;

        let records: RefCell<Vec<SmbiosRecord>> = RefCell::new(vec![]);
        let borrowed = records.borrow();
        let mut iter = SmbiosRecordsIter::new(borrowed, None);

        assert!(iter.next().is_none());
    }

    #[test]
    fn test_smbios_records_iter_no_match_filter() {
        use crate::manager::SmbiosRecord;
        use alloc::vec;
        use core::cell::RefCell;

        let records = RefCell::new(vec![
            SmbiosRecord::new(SmbiosTableHeader::new(0, 24, 0x0001), None, vec![], 0),
            SmbiosRecord::new(SmbiosTableHeader::new(1, 32, 0x0002), None, vec![], 0),
        ]);

        let borrowed = records.borrow();
        let mut iter = SmbiosRecordsIter::new(borrowed, Some(127)); // Filter for type 127

        assert!(iter.next().is_none());
    }

    // Mock-based tests demonstrating mockability of Smbios trait object

    /// Mock implementation of Smbios for testing
    struct MockSmbios {
        version: (u8, u8),
        add_from_bytes_result: core::result::Result<SmbiosHandle, crate::error::SmbiosError>,
        expected_bytes: Option<Vec<u8>>,
    }

    impl Smbios for MockSmbios {
        fn version(&self) -> (u8, u8) {
            self.version
        }

        fn publish_table(
            &self,
        ) -> core::result::Result<(r_efi::efi::PhysicalAddress, r_efi::efi::PhysicalAddress), crate::error::SmbiosError>
        {
            Ok((0x1000, 0x2000))
        }

        fn update_string(
            &self,
            _smbios_handle: SmbiosHandle,
            _string_number: usize,
            _string: &str,
        ) -> core::result::Result<(), crate::error::SmbiosError> {
            Ok(())
        }

        fn remove(&self, _smbios_handle: SmbiosHandle) -> core::result::Result<(), crate::error::SmbiosError> {
            Ok(())
        }

        fn add_from_bytes(
            &self,
            _producer_handle: Option<r_efi::efi::Handle>,
            bytes: &[u8],
        ) -> core::result::Result<SmbiosHandle, crate::error::SmbiosError> {
            // Verify expected bytes if provided
            if let Some(ref expected) = self.expected_bytes {
                assert_eq!(bytes, expected.as_slice(), "add_from_bytes received unexpected bytes");
            }
            self.add_from_bytes_result.clone()
        }
    }

    #[test]
    fn test_mock_smbios_service_version() {
        use patina::component::service::Service;

        let mock = MockSmbios { version: (99, 88), add_from_bytes_result: Ok(0x1234), expected_bytes: None };
        let service: Service<dyn Smbios> = Service::mock(Box::new(mock));

        assert_eq!(service.version(), (99, 88));
    }

    #[test]
    fn test_mock_smbios_service_add_from_bytes() {
        use patina::component::service::Service;

        let test_bytes = vec![127, 4, 0xFE, 0xFF, 0, 0]; // Type 127, length 4, handle 0xFFFE

        let mock =
            MockSmbios { version: (3, 0), add_from_bytes_result: Ok(0x5678), expected_bytes: Some(test_bytes.clone()) };
        let service: Service<dyn Smbios> = Service::mock(Box::new(mock));

        let result = service.add_from_bytes(None, &test_bytes);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0x5678);
    }

    #[test]
    fn test_mock_smbios_service_add_from_bytes_error() {
        use patina::component::service::Service;

        let mock = MockSmbios {
            version: (3, 0),
            add_from_bytes_result: Err(crate::error::SmbiosError::RecordTooSmall),
            expected_bytes: None,
        };
        let service: Service<dyn Smbios> = Service::mock(Box::new(mock));

        let result = service.add_from_bytes(None, &[1, 2, 3]);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), crate::error::SmbiosError::RecordTooSmall));
    }

    #[test]
    fn test_mock_smbios_service_add_record_integration() {
        use crate::smbios_record::{SmbiosRecordStructure, Type127EndOfTable};
        use alloc::vec;
        use patina::component::service::Service;

        // Create a test record
        let record = Type127EndOfTable { header: SmbiosTableHeader::new(127, 4, 0xFFFE), string_pool: vec![] };

        // Serialize it to get expected bytes
        let expected_bytes = record.to_bytes();

        // Create mock that expects these exact bytes
        let mock =
            MockSmbios { version: (3, 0), add_from_bytes_result: Ok(0xABCD), expected_bytes: Some(expected_bytes) };
        let service: Service<dyn Smbios> = Service::mock(Box::new(mock));

        // Verify the mock works through Service
        let result = service.add_from_bytes(None, &record.to_bytes());
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0xABCD);
    }

    #[test]
    fn test_mock_smbios_service_all_trait_methods() {
        use patina::component::service::Service;

        let mock = MockSmbios { version: (3, 7), add_from_bytes_result: Ok(0x1111), expected_bytes: None };
        let service: Service<dyn Smbios> = Service::mock(Box::new(mock));

        // Test all trait methods
        assert_eq!(service.version(), (3, 7));
        assert!(service.publish_table().is_ok());
        assert!(service.update_string(0x1234, 1, "test").is_ok());
        assert!(service.remove(0x1234).is_ok());
        assert!(service.add_from_bytes(None, &[127, 4, 0, 0, 0, 0]).is_ok());
    }

    #[test]
    fn test_mock_add_record_extension_trait_pattern() {
        use crate::smbios_record::{SmbiosRecordStructure, Type127EndOfTable};
        use alloc::vec;
        use patina::component::service::Service;

        // Create a test record
        let record = Type127EndOfTable { header: SmbiosTableHeader::new(127, 4, 0xFFFE), string_pool: vec![] };

        // Serialize to get expected bytes
        let expected_bytes = record.to_bytes();

        // Create mock that verifies the bytes and returns a handle
        let mock = MockSmbios {
            version: (3, 0),
            add_from_bytes_result: Ok(0x9999),
            expected_bytes: Some(expected_bytes.clone()),
        };
        let service: Service<dyn Smbios> = Service::mock(Box::new(mock));

        // Demonstrate how add_record<T>() delegates to add_from_bytes()
        // The extension trait does:
        //   1. let bytes = record.to_bytes();
        //   2. self.add_from_bytes(producer_handle, &bytes)
        let result = service.add_from_bytes(None, &expected_bytes);

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0x9999);
        // Mock verified the bytes matched expected_bytes in add_from_bytes()
    }

    #[test]
    fn test_mock_add_record_with_error() {
        use crate::smbios_record::{SmbiosRecordStructure, Type127EndOfTable};
        use alloc::vec;
        use patina::component::service::Service;

        // Mock that returns an error
        let mock = MockSmbios {
            version: (3, 0),
            add_from_bytes_result: Err(crate::error::SmbiosError::HandleExhausted),
            expected_bytes: None,
        };
        let service: Service<dyn Smbios> = Service::mock(Box::new(mock));

        // Simulate add_record<T>() behavior
        let record = Type127EndOfTable { header: SmbiosTableHeader::new(127, 4, 0xFFFE), string_pool: vec![] };
        let bytes = record.to_bytes();

        let result = service.add_from_bytes(None, &bytes);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), crate::error::SmbiosError::HandleExhausted));
    }

    #[test]
    fn test_mock_multiple_record_types() {
        use crate::smbios_record::{SmbiosRecordStructure, Type0PlatformFirmwareInformation, Type127EndOfTable};
        use alloc::{string::String, vec};
        use patina::component::service::Service;

        // Test that mock can handle different record types
        let type0 = Type0PlatformFirmwareInformation {
            header: SmbiosTableHeader::new(0, 24, 0xFFFE),
            vendor: 1,
            firmware_version: 2,
            bios_starting_address_segment: 0xE800,
            firmware_release_date: 3,
            firmware_rom_size: 0xFF,
            characteristics: 0x08,
            characteristics_ext1: 0x03,
            characteristics_ext2: 0x03,
            system_bios_major_release: 1,
            system_bios_minor_release: 0,
            embedded_controller_major_release: 0xFF,
            embedded_controller_minor_release: 0xFF,
            extended_bios_rom_size: 0,
            string_pool: vec![String::from("Vendor"), String::from("1.0"), String::from("2025")],
        };

        let type127 = Type127EndOfTable { header: SmbiosTableHeader::new(127, 4, 0xFFFE), string_pool: vec![] };

        // Mock returns different handles for different records
        let mock_type0 =
            MockSmbios { version: (3, 0), add_from_bytes_result: Ok(0x0001), expected_bytes: Some(type0.to_bytes()) };
        let service_type0: Service<dyn Smbios> = Service::mock(Box::new(mock_type0));

        let mock_type127 =
            MockSmbios { version: (3, 0), add_from_bytes_result: Ok(0x0002), expected_bytes: Some(type127.to_bytes()) };
        let service_type127: Service<dyn Smbios> = Service::mock(Box::new(mock_type127));

        // Verify different records work with mock
        let result0 = service_type0.add_from_bytes(None, &type0.to_bytes());
        assert_eq!(result0.unwrap(), 0x0001);

        let result127 = service_type127.add_from_bytes(None, &type127.to_bytes());
        assert_eq!(result127.unwrap(), 0x0002);
    }

    #[test]
    fn test_service_mock_pattern() {
        use crate::smbios_record::{SmbiosRecordStructure, Type127EndOfTable};
        use alloc::vec;
        use patina::component::service::Service;

        // Create a mock service using the standard Service::mock pattern
        let mock = MockSmbios { version: (3, 6), add_from_bytes_result: Ok(0xBEEF), expected_bytes: None };

        // This is the pattern used throughout the Patina codebase for testing
        let service: Service<dyn Smbios> = Service::mock(Box::new(mock));

        // Verify trait methods work through Service
        assert_eq!(service.version(), (3, 6));
        assert!(service.publish_table().is_ok());

        // Verify extension trait methods work through Service
        let record = Type127EndOfTable { header: SmbiosTableHeader::new(127, 4, 0xFFFE), string_pool: vec![] };
        let result = service.add_from_bytes(None, &record.to_bytes());
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0xBEEF);
    }

    #[test]
    fn test_service_mock_with_extension_trait() {
        use crate::smbios_record::{SmbiosRecordStructure, Type127EndOfTable};
        use alloc::vec;
        use patina::component::service::Service;

        // Create test record
        let record = Type127EndOfTable { header: SmbiosTableHeader::new(127, 4, 0xFFFE), string_pool: vec![] };
        let expected_bytes = record.to_bytes();

        // Mock that validates bytes and returns handle
        let mock =
            MockSmbios { version: (3, 0), add_from_bytes_result: Ok(0x1337), expected_bytes: Some(expected_bytes) };

        let service: Service<dyn Smbios> = Service::mock(Box::new(mock));

        // The extension trait method add_record<T>() works through Service
        // It serializes the record and calls add_from_bytes (which the mock implements)
        let handle = service.add_record(None, &record).unwrap();
        assert_eq!(handle, 0x1337);
    }
}
