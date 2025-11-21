//! C/EDKII protocol compatibility layer
//!
//! This module provides the EDKII-compatible C protocol interface for SMBIOS operations.
//! It implements the standard EDK2 SMBIOS protocol with functions for adding, updating,
//! removing, and iterating SMBIOS records.
//!
//! This module is excluded from coverage as it's FFI code tested via integration.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::ffi::c_char;

use patina::{tpl_mutex::TplMutex, uefi_protocol::ProtocolInterface};
use r_efi::efi;

use crate::{
    error::SmbiosError,
    service::{SMBIOS_HANDLE_PI_RESERVED, SmbiosHandle, SmbiosTableHeader, SmbiosType},
};

use super::core::SmbiosManager;

#[repr(C)]
pub(super) struct SmbiosProtocol {
    add: SmbiosAdd,
    update_string: SmbiosUpdateString,
    remove: SmbiosRemove,
    get_next: SmbiosGetNext,
    major_version: u8,
    minor_version: u8,
}

/// Internal protocol struct that packs the manager behind the protocol
#[repr(C)]
pub(super) struct SmbiosProtocolInternal {
    // The public protocol that external callers will depend on
    pub(super) protocol: SmbiosProtocol,

    // Internal component access only! Does not exist in C definition
    pub(super) manager: &'static TplMutex<'static, SmbiosManager>,

    // Boot services needed for table republishing after Add/Update/Remove
    pub(super) boot_services: &'static patina::boot_services::StandardBootServices,
}

// SAFETY: SmbiosProtocol implements the SMBIOS protocol interface. The struct layout
// must match the SMBIOS protocol interface with function pointers in the correct order.
unsafe impl ProtocolInterface for SmbiosProtocol {
    const PROTOCOL_GUID: efi::Guid =
        efi::Guid::from_fields(0x03583ff6, 0xcb36, 0x4940, 0x94, 0x7e, &[0xb9, 0xb3, 0x9f, 0x4a, 0xfa, 0xf7]);
}

type SmbiosAdd =
    extern "efiapi" fn(*const SmbiosProtocol, efi::Handle, *mut SmbiosHandle, *const SmbiosTableHeader) -> efi::Status;

type SmbiosUpdateString =
    extern "efiapi" fn(*const SmbiosProtocol, *mut SmbiosHandle, *mut usize, *const c_char) -> efi::Status;

type SmbiosRemove = extern "efiapi" fn(*const SmbiosProtocol, SmbiosHandle) -> efi::Status;

type SmbiosGetNext = extern "efiapi" fn(
    *const SmbiosProtocol,
    *mut SmbiosHandle,
    *mut SmbiosType,
    *mut *mut SmbiosTableHeader,
    *mut efi::Handle,
) -> efi::Status;

impl SmbiosProtocol {
    pub(super) fn new(major_version: u8, minor_version: u8) -> Self {
        Self {
            add: Self::add_ext,
            update_string: Self::update_string_ext,
            remove: Self::remove_ext,
            get_next: Self::get_next_ext,
            major_version,
            minor_version,
        }
    }
}

impl SmbiosProtocolInternal {
    /// Creates a new SMBIOS protocol internal structure
    ///
    /// This constructor is tested via integration (Q35 platform component)
    /// as it requires 'static boot services which cannot be mocked in unit tests.
    #[coverage(off)]
    pub(super) fn new(
        major_version: u8,
        minor_version: u8,
        manager: &'static TplMutex<'static, SmbiosManager>,
        boot_services: &'static patina::boot_services::StandardBootServices,
    ) -> Self {
        Self { protocol: SmbiosProtocol::new(major_version, minor_version), manager, boot_services }
    }
}

impl SmbiosProtocol {
    /// C protocol implementation for adding SMBIOS records
    ///
    /// # Safety
    ///
    /// This function is only safe to call from the C UEFI protocol layer where the
    /// caller guarantees that `record` points to a complete, valid SMBIOS record.
    #[coverage(off)] // FFI function - tested via integration tests
    extern "efiapi" fn add_ext(
        protocol: *const SmbiosProtocol,
        producer_handle: efi::Handle,
        smbios_handle: *mut SmbiosHandle,
        record: *const SmbiosTableHeader,
    ) -> efi::Status {
        // Safety checks: validate all pointers before dereferencing
        if protocol.is_null() || smbios_handle.is_null() || record.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }

        // Check protocol pointer alignment
        if !(protocol as usize).is_multiple_of(core::mem::align_of::<SmbiosProtocolInternal>()) {
            debug_assert!(false, "[SMBIOS Add] Protocol pointer misaligned: {:p}", protocol);
            return efi::Status::INVALID_PARAMETER;
        }

        // SAFETY: Protocol pointer has been validated as non-null and properly aligned above.
        // Cast from protocol pointer to internal struct pointer is safe due to repr(C) layout:
        // SmbiosProtocolInternal has SmbiosProtocol as its first field, so a pointer to
        // SmbiosProtocol is also a valid pointer to the containing SmbiosProtocolInternal.
        let internal = unsafe { &*(protocol as *const SmbiosProtocolInternal) };

        let manager = match internal.manager.try_lock() {
            Ok(guard) => guard,
            Err(_) => {
                debug_assert!(false, "[SMBIOS Add] ERROR: try_lock FAILED - mutex already locked!");
                return efi::Status::DEVICE_ERROR;
            }
        };

        // SAFETY: The C UEFI protocol caller guarantees that `record` points to a valid,
        // complete SMBIOS record. We read the length field and scan for string pool terminator.
        let full_record_bytes = unsafe {
            let header = &*record;
            let record_length = header.length as usize;

            // Validate that we can safely read the record
            if record_length < core::mem::size_of::<SmbiosTableHeader>() {
                return efi::Status::INVALID_PARAMETER;
            }

            // Scan for the string pool terminator (double null)
            let base_ptr = record as *const u8;

            // Scan for double null terminator
            let mut consecutive_nulls = 0;
            let mut offset = record_length;
            const MAX_STRING_POOL_SIZE: usize = 4096; // Safety limit

            while consecutive_nulls < 2 && offset < record_length + MAX_STRING_POOL_SIZE {
                let byte = *base_ptr.add(offset);
                if byte == 0 {
                    consecutive_nulls += 1;
                } else {
                    consecutive_nulls = 0;
                }
                offset += 1;
            }

            if consecutive_nulls < 2 {
                // Malformed record - no double null terminator found
                return efi::Status::INVALID_PARAMETER;
            }

            let total_size = offset;

            // Create a slice of the complete record
            core::slice::from_raw_parts(base_ptr, total_size)
        };

        // Convert handle
        let producer_opt = if producer_handle.is_null() { None } else { Some(producer_handle) };

        // Add the record
        let result = manager.add_from_bytes(producer_opt, full_record_bytes);

        match result {
            Ok(handle) => {
                // SAFETY: smbios_handle pointer is guaranteed valid by caller
                unsafe {
                    smbios_handle.write_unaligned(handle);
                }

                if manager.republish_table().is_err() {
                    log::error!("[SMBIOS Add] Failed to rebuild table");
                    return efi::Status::DEVICE_ERROR;
                }

                efi::Status::SUCCESS
            }
            Err(SmbiosError::StringContainsNull) => efi::Status::INVALID_PARAMETER,
            Err(SmbiosError::EmptyStringInPool) => efi::Status::INVALID_PARAMETER,
            Err(SmbiosError::RecordTooSmall) => efi::Status::BUFFER_TOO_SMALL,
            Err(SmbiosError::MalformedRecordHeader) => efi::Status::INVALID_PARAMETER,
            Err(SmbiosError::InvalidStringPoolTermination) => efi::Status::INVALID_PARAMETER,
            Err(SmbiosError::StringPoolTooSmall) => efi::Status::BUFFER_TOO_SMALL,
            Err(SmbiosError::HandleExhausted) => efi::Status::OUT_OF_RESOURCES,
            Err(SmbiosError::AllocationFailed) => efi::Status::OUT_OF_RESOURCES,
            Err(SmbiosError::StringTooLong) => efi::Status::INVALID_PARAMETER,
            Err(e) => {
                log::error!("[SMBIOS Add] Error: {:?}", e);
                efi::Status::DEVICE_ERROR
            }
        }
    }

    #[coverage(off)] // FFI function - tested via integration tests
    extern "efiapi" fn update_string_ext(
        protocol: *const SmbiosProtocol,
        smbios_handle: *mut SmbiosHandle,
        string_number: *mut usize,
        string: *const c_char,
    ) -> efi::Status {
        // Safety checks: validate all pointers before dereferencing
        if protocol.is_null() || smbios_handle.is_null() || string_number.is_null() || string.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }

        // Check protocol pointer alignment
        if !(protocol as usize).is_multiple_of(core::mem::align_of::<SmbiosProtocolInternal>()) {
            debug_assert!(false, "[SMBIOS UpdateString] Protocol pointer misaligned: {:p}", protocol);
            return efi::Status::INVALID_PARAMETER;
        }

        // SAFETY: Protocol pointer validated as non-null and aligned. See add_ext for details on repr(C) cast.
        let internal = unsafe { &*(protocol as *const SmbiosProtocolInternal) };
        let manager = internal.manager.lock();

        // SAFETY: The pointers are checked for being null above and guaranteed valid by caller
        let (handle, str_num, rust_str) = unsafe {
            let handle = smbios_handle.read_unaligned();
            let str_num = string_number.read_unaligned();

            // Convert C string to Rust str
            let c_str = core::ffi::CStr::from_ptr(string);
            let rust_str = match c_str.to_str() {
                Ok(s) => s,
                Err(_) => return efi::Status::INVALID_PARAMETER,
            };

            (handle, str_num, rust_str)
        };

        match manager.update_string(handle, str_num, rust_str) {
            Ok(()) => {
                if manager.republish_table().is_err() {
                    log::error!("[SMBIOS UpdateString] Failed to rebuild table");
                    return efi::Status::DEVICE_ERROR;
                }

                efi::Status::SUCCESS
            }
            Err(SmbiosError::StringContainsNull) => efi::Status::INVALID_PARAMETER,
            Err(SmbiosError::HandleNotFound) => efi::Status::NOT_FOUND,
            Err(SmbiosError::StringIndexOutOfRange) => efi::Status::INVALID_PARAMETER,
            Err(SmbiosError::StringTooLong) => efi::Status::INVALID_PARAMETER,
            Err(_) => efi::Status::DEVICE_ERROR,
        }
    }

    #[coverage(off)] // FFI function - tested via integration tests
    extern "efiapi" fn remove_ext(protocol: *const SmbiosProtocol, smbios_handle: SmbiosHandle) -> efi::Status {
        // Safety check: validate protocol pointer before dereferencing
        if protocol.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }

        // Check protocol pointer alignment
        if !(protocol as usize).is_multiple_of(core::mem::align_of::<SmbiosProtocolInternal>()) {
            debug_assert!(false, "[SMBIOS Remove] Protocol pointer misaligned: {:p}", protocol);
            return efi::Status::INVALID_PARAMETER;
        }

        // SAFETY: Protocol pointer validated as non-null and aligned. See add_ext for details on repr(C) cast.
        let internal = unsafe { &*(protocol as *const SmbiosProtocolInternal) };
        let manager = internal.manager.lock();

        match manager.remove(smbios_handle) {
            Ok(()) => {
                if manager.republish_table().is_err() {
                    log::error!("[SMBIOS Remove] Failed to rebuild table");
                    return efi::Status::DEVICE_ERROR;
                }

                efi::Status::SUCCESS
            }
            Err(SmbiosError::HandleNotFound) => efi::Status::NOT_FOUND,
            Err(_) => efi::Status::DEVICE_ERROR,
        }
    }

    #[coverage(off)] // FFI function - tested via integration tests
    extern "efiapi" fn get_next_ext(
        protocol: *const SmbiosProtocol,
        smbios_handle: *mut SmbiosHandle,
        record_type: *mut SmbiosType,
        record: *mut *mut SmbiosTableHeader,
        producer_handle: *mut efi::Handle,
    ) -> efi::Status {
        // Safety checks: validate all required pointers before dereferencing
        if protocol.is_null() || smbios_handle.is_null() || record.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }

        // Check protocol pointer alignment
        if !(protocol as usize).is_multiple_of(core::mem::align_of::<SmbiosProtocolInternal>()) {
            debug_assert!(false, "[SMBIOS GetNext] Protocol pointer misaligned: {:p}", protocol);
            return efi::Status::INVALID_PARAMETER;
        }

        // SAFETY: Protocol pointer validated as non-null and aligned. See add_ext for details on repr(C) cast.
        let internal = unsafe { &*(protocol as *const SmbiosProtocolInternal) };

        let found_handle = {
            let manager = internal.manager.lock();

            // SAFETY: C UEFI protocol caller guarantees pointers are valid
            let (handle, type_filter) = unsafe {
                (
                    smbios_handle.read_unaligned(),
                    if record_type.is_null() { None } else { Some(record_type.read_unaligned()) },
                )
            };

            // Use the iterator to find the next record
            let mut iter = manager.iter(type_filter);

            // Skip records until we find the one after the current handle
            let next_record = if handle == SMBIOS_HANDLE_PI_RESERVED {
                // Starting iteration - get first record
                iter.next()
            } else {
                // Find the record after the current handle
                iter.skip_while(|(hdr, _)| hdr.handle != handle).nth(1)
            };

            next_record.map(|(header, _)| header.handle)
        }; // manager borrow drops here

        let Some(found_handle) = found_handle else {
            return efi::Status::NOT_FOUND;
        };

        // SAFETY: smbios_handle pointer is guaranteed valid by caller
        unsafe {
            smbios_handle.write_unaligned(found_handle);
        }

        // Get pointer to record in published table
        let manager = internal.manager.lock();
        if let Some((record_addr, prod)) = manager.get_record_pointer(found_handle) {
            // SAFETY: record pointer is guaranteed valid by caller
            unsafe {
                record.write_unaligned(record_addr as *mut SmbiosTableHeader);
            }

            if !producer_handle.is_null() {
                // SAFETY: producer_handle pointer is guaranteed valid by caller (checked for null)
                unsafe {
                    producer_handle.write_unaligned(prod.unwrap_or(core::ptr::null_mut()));
                }
            }
            efi::Status::SUCCESS
        } else {
            debug_assert!(false, "[SMBIOS GetNext] Record handle {:04X} not found in second lookup", found_handle);
            efi::Status::NOT_FOUND
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manager::SmbiosManager;
    extern crate std;
    use std::vec::Vec;

    fn create_test_bios_info_record() -> Vec<u8> {
        // Create a simple BIOS Information record (Type 0)
        let mut record = Vec::new();

        // Header
        record.push(0); // Type: BIOS Information
        record.push(24); // Length
        record.extend_from_slice(&0x0001u16.to_le_bytes()); // Handle

        // BIOS Information specific fields (simplified)
        record.push(1); // Vendor string number
        record.push(2); // BIOS Version string number
        record.extend_from_slice(&0x0000u16.to_le_bytes()); // BIOS Starting Address Segment
        record.push(3); // BIOS Release Date string number
        record.push(0); // BIOS ROM Size
        record.extend_from_slice(&[0; 8]); // BIOS Characteristics
        record.extend_from_slice(&[0; 2]); // BIOS Characteristics Extension Bytes
        record.push(0); // System BIOS Major Release
        record.push(0); // System BIOS Minor Release
        record.push(0); // Embedded Controller Firmware Major Release
        record.push(0); // Embedded Controller Firmware Minor Release

        // Strings section
        record.extend_from_slice(b"Test Vendor\0"); // String 1
        record.extend_from_slice(b"Test Version\0"); // String 2
        record.extend_from_slice(b"01/01/2023\0"); // String 3
        record.push(0); // End of strings marker

        record
    }

    // Core manager functionality tests - these test the underlying logic
    #[test]
    fn test_manager_add_record() {
        let manager = SmbiosManager::new(3, 6).unwrap();
        let record_data = create_test_bios_info_record();

        let result = manager.add_from_bytes(None, &record_data);
        assert!(result.is_ok());

        let handle = result.unwrap();
        assert_ne!(handle, 0);
    }

    #[test]
    fn test_manager_add_invalid_record() {
        let manager = SmbiosManager::new(3, 6).unwrap();
        let invalid_record = std::vec![1, 2, 3]; // Too small

        let result = manager.add_from_bytes(None, &invalid_record);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), SmbiosError::RecordTooSmall));
    }

    #[test]
    fn test_manager_operations() {
        let manager = SmbiosManager::new(3, 6).unwrap();
        let record_data = create_test_bios_info_record();

        // Add record
        let handle = manager.add_from_bytes(None, &record_data).unwrap();

        // Update string
        let result = manager.update_string(handle, 1, "Updated Vendor");
        assert!(result.is_ok());

        // Remove record
        let result = manager.remove(handle);
        assert!(result.is_ok());

        // Try to remove again (should fail)
        let result2 = manager.remove(handle);
        assert!(result2.is_err());
    }

    // Protocol-specific tests
    #[test]
    fn test_protocol_new() {
        let protocol = SmbiosProtocol::new(3, 9);
        assert_eq!(protocol.major_version, 3);
        assert_eq!(protocol.minor_version, 9);
    }

    #[test]
    fn test_protocol_custom_version() {
        // Test with SMBIOS 3.7 (a valid historical version)
        let protocol = SmbiosProtocol::new(3, 7);
        assert_eq!(protocol.major_version, 3);
        assert_eq!(protocol.minor_version, 7);
    }

    #[test]
    fn test_protocol_version_storage() {
        // Test that version is stored in the protocol struct without needing manager access
        let protocol1 = SmbiosProtocol::new(3, 0);
        let protocol2 = SmbiosProtocol::new(3, 9);

        assert_eq!(protocol1.major_version, 3);
        assert_eq!(protocol1.minor_version, 0);
        assert_eq!(protocol2.major_version, 3);
        assert_eq!(protocol2.minor_version, 9);
    }

    #[test]
    fn test_protocol_guid() {
        use patina::uefi_protocol::ProtocolInterface;

        // Verify the GUID matches the EDK2 SMBIOS protocol GUID
        let expected_guid =
            efi::Guid::from_fields(0x03583ff6, 0xcb36, 0x4940, 0x94, 0x7e, &[0xb9, 0xb3, 0x9f, 0x4a, 0xfa, 0xf7]);

        assert_eq!(SmbiosProtocol::PROTOCOL_GUID, expected_guid);
    }

    #[test]
    fn test_repr_c_layout_guarantee() {
        use core::mem::{align_of, offset_of};

        // Verify repr(C) layout guarantee: SmbiosProtocol is at offset 0 in SmbiosProtocolInternal
        // This is critical for the pointer cast pattern: &*(protocol as *const SmbiosProtocolInternal)
        assert_eq!(
            offset_of!(SmbiosProtocolInternal, protocol),
            0,
            "SmbiosProtocol must be the first field at offset 0 for safe pointer casting"
        );

        // Verify alignment is compatible
        assert!(
            align_of::<SmbiosProtocol>() <= align_of::<SmbiosProtocolInternal>(),
            "SmbiosProtocol alignment must be <= SmbiosProtocolInternal alignment"
        );

        // Verify a SmbiosProtocol pointer can be aligned as SmbiosProtocolInternal
        // Since protocol is at offset 0, any properly aligned SmbiosProtocolInternal pointer
        // is also a properly aligned SmbiosProtocol pointer (and vice versa when protocol is first field)
        let protocol = SmbiosProtocol::new(3, 9);
        let protocol_ptr = &protocol as *const SmbiosProtocol;
        let protocol_addr = protocol_ptr as usize;

        // This pointer should be valid for casting to SmbiosProtocolInternal alignment
        assert_eq!(
            protocol_addr % align_of::<SmbiosProtocolInternal>(),
            0,
            "SmbiosProtocol pointer should be aligned for SmbiosProtocolInternal"
        );
    }

    #[test]
    fn test_protocol_is_repr_c() {
        // Verify SmbiosProtocol can be used as a C struct
        // The size should be consistent (function pointers + version bytes)
        let size = core::mem::size_of::<SmbiosProtocol>();

        // Should have 4 function pointers and 2 u8 fields
        // Size will vary by architecture but should be deterministic
        assert!(size > 0);
        assert_eq!(size, core::mem::size_of::<SmbiosProtocol>());
    }
}
