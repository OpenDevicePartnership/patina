//! C/EDKII protocol compatibility layer
//!
//! This module is excluded from coverage as it's FFI code tested via integration.

use core::ffi::c_char;

use patina::uefi_protocol::ProtocolInterface;
use r_efi::efi;

use crate::error::SmbiosError;
use crate::service::{SMBIOS_HANDLE_PI_RESERVED, SmbiosHandle, SmbiosTableHeader, SmbiosType};

use super::global::{SMBIOS_HEADER_BUFFER, SMBIOS_MANAGER};

#[repr(C)]
#[allow(dead_code)]
pub(super) struct SmbiosProtocol {
    add: SmbiosAdd,
    update_string: SmbiosUpdateString,
    remove: SmbiosRemove,
    get_next: SmbiosGetNext,
    major_version: u8,
    minor_version: u8,
}

unsafe impl ProtocolInterface for SmbiosProtocol {
    const PROTOCOL_GUID: efi::Guid =
        efi::Guid::from_fields(0x03583ff6, 0xcb36, 0x4940, 0x94, 0x7e, &[0xb9, 0xb3, 0x9f, 0x4a, 0xfa, 0xf7]);
}

#[allow(dead_code)]
type SmbiosAdd =
    extern "efiapi" fn(*const SmbiosProtocol, efi::Handle, *mut SmbiosHandle, *const SmbiosTableHeader) -> efi::Status;

#[allow(dead_code)]
type SmbiosUpdateString =
    extern "efiapi" fn(*const SmbiosProtocol, *mut SmbiosHandle, *mut usize, *const c_char) -> efi::Status;

#[allow(dead_code)]
type SmbiosRemove = extern "efiapi" fn(*const SmbiosProtocol, SmbiosHandle) -> efi::Status;

#[allow(dead_code)]
type SmbiosGetNext = extern "efiapi" fn(
    *const SmbiosProtocol,
    *mut SmbiosHandle,
    *mut SmbiosType,
    *mut *mut SmbiosTableHeader,
    *mut efi::Handle,
) -> efi::Status;

impl SmbiosProtocol {
    #[allow(dead_code)]
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

    /// C protocol implementation for adding SMBIOS records
    ///
    /// # Safety
    ///
    /// This function is only safe to call from the C UEFI protocol layer where the
    /// caller guarantees that `record` points to a complete, valid SMBIOS record.
    #[allow(dead_code)]
    #[coverage(off)] // FFI function - tested via integration tests
    extern "efiapi" fn add_ext(
        _protocol: *const SmbiosProtocol,
        producer_handle: efi::Handle,
        smbios_handle: *mut SmbiosHandle,
        record: *const SmbiosTableHeader,
    ) -> efi::Status {
        // Safety checks
        if smbios_handle.is_null() || record.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }

        // Get the global manager
        // SAFETY: We're just checking if the manager exists
        let tpl_mutex = match unsafe { SMBIOS_MANAGER.get() } {
            Some(m) => m,
            None => return efi::Status::NOT_READY,
        };

        let manager = tpl_mutex.lock();

        // SAFETY: The C UEFI protocol caller guarantees that `record` points to a valid,
        // complete SMBIOS record. We read the length field to determine the full record size.
        unsafe {
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
            let full_record_bytes = core::slice::from_raw_parts(base_ptr, total_size);

            // Convert handle
            let producer_opt = if producer_handle.is_null() { None } else { Some(producer_handle) };

            // Add the record
            match manager.add_from_bytes(producer_opt, full_record_bytes) {
                Ok(handle) => {
                    *smbios_handle = handle;
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
                Err(_) => efi::Status::DEVICE_ERROR,
            }
        }
    }

    #[allow(dead_code)]
    #[coverage(off)] // FFI function - tested via integration tests
    extern "efiapi" fn update_string_ext(
        _protocol: *const SmbiosProtocol,
        smbios_handle: *mut SmbiosHandle,
        string_number: *mut usize,
        string: *const c_char,
    ) -> efi::Status {
        if smbios_handle.is_null() || string_number.is_null() || string.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }

        // Get the global TplMutex and lock it (raises TPL to NOTIFY)
        let tpl_mutex = match unsafe { SMBIOS_MANAGER.get() } {
            Some(m) => m,
            None => return efi::Status::NOT_READY,
        };

        let manager = tpl_mutex.lock();

        unsafe {
            let handle = *smbios_handle;
            let str_num = *string_number;

            // Convert C string to Rust str
            let c_str = core::ffi::CStr::from_ptr(string);
            let rust_str = match c_str.to_str() {
                Ok(s) => s,
                Err(_) => return efi::Status::INVALID_PARAMETER,
            };

            match manager.update_string(handle, str_num, rust_str) {
                Ok(()) => efi::Status::SUCCESS,
                Err(SmbiosError::StringContainsNull) => efi::Status::INVALID_PARAMETER,
                Err(SmbiosError::HandleNotFound) => efi::Status::NOT_FOUND,
                Err(SmbiosError::StringIndexOutOfRange) => efi::Status::INVALID_PARAMETER,
                Err(SmbiosError::StringTooLong) => efi::Status::INVALID_PARAMETER,
                Err(_) => efi::Status::DEVICE_ERROR,
            }
        }
    }

    #[allow(dead_code)]
    #[coverage(off)] // FFI function - tested via integration tests
    extern "efiapi" fn remove_ext(_protocol: *const SmbiosProtocol, smbios_handle: SmbiosHandle) -> efi::Status {
        let tpl_mutex = match unsafe { SMBIOS_MANAGER.get() } {
            Some(m) => m,
            None => return efi::Status::NOT_READY,
        };

        let manager = tpl_mutex.lock();

        match manager.remove(smbios_handle) {
            Ok(()) => efi::Status::SUCCESS,
            Err(SmbiosError::HandleNotFound) => efi::Status::NOT_FOUND,
            Err(_) => efi::Status::DEVICE_ERROR,
        }
    }

    #[allow(dead_code)]
    #[coverage(off)] // FFI function - tested via integration tests
    extern "efiapi" fn get_next_ext(
        _protocol: *const SmbiosProtocol,
        smbios_handle: *mut SmbiosHandle,
        record_type: *mut SmbiosType,
        record: *mut *mut SmbiosTableHeader,
        producer_handle: *mut efi::Handle,
    ) -> efi::Status {
        if smbios_handle.is_null() || record.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }

        // Get the global TplMutex and lock it (raises TPL to NOTIFY)
        let tpl_mutex = match unsafe { SMBIOS_MANAGER.get() } {
            Some(m) => m,
            None => return efi::Status::NOT_READY,
        };

        let manager = tpl_mutex.lock();

        unsafe {
            let handle = *smbios_handle;
            let type_filter = if record_type.is_null() { None } else { Some(*record_type) };

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

            match next_record {
                Some((header_value, prod_handle)) => {
                    *smbios_handle = header_value.handle;

                    // Store header in static buffer and return pointer to it.
                    let buffer_ptr = SMBIOS_HEADER_BUFFER.get();
                    *buffer_ptr = header_value;
                    *record = buffer_ptr;

                    if !producer_handle.is_null() {
                        *producer_handle = prod_handle.unwrap_or(core::ptr::null_mut());
                    }
                    efi::Status::SUCCESS
                }
                None => {
                    *smbios_handle = SMBIOS_HANDLE_PI_RESERVED;
                    efi::Status::NOT_FOUND
                }
            }
        }
    }
}

#[cfg(test)]
#[coverage(off)]
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
}
