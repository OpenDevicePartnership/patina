//! ACPI C Protocol Definitions.
//!
//! Wrappers for the C ACPI protocols to call into Rust ACPI implementations.
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use crate::{
    acpi_table::{AcpiTable, AcpiTableHeader},
    signature::{self, ACPI_VERSIONS_GTE_2},
};

use core::{ffi::c_void, mem};
use patina::uefi_protocol::ProtocolInterface;
use r_efi::efi;

use crate::{
    acpi::STANDARD_ACPI_PROVIDER,
    service::{AcpiNotifyFn, AcpiProvider, TableKey},
};

/// Corresponds to the ACPI Table Protocol as defined in UEFI spec.
#[repr(C)]
pub struct AcpiTableProtocol {
    pub install_table: AcpiTableInstall,
    pub uninstall_table: AcpiTableUninstall,
}

// SAFETY: `AcpiTableProtocol` matches the C layout and behavior of the EFI_ACPI_TABLE_PROTOCOL.
unsafe impl ProtocolInterface for AcpiTableProtocol {
    const PROTOCOL_GUID: patina::BinaryGuid = patina::BinaryGuid::from_string("FFE06BDD-6107-46A6-7BB2-5A9C7EC5275C");
}

// C function interfaces for ACPI Table Protocol and ACPI Get Protocol.
type AcpiTableInstall = extern "efiapi" fn(*const AcpiTableProtocol, *const c_void, usize, *mut usize) -> efi::Status;
type AcpiTableUninstall = extern "efiapi" fn(*const AcpiTableProtocol, usize) -> efi::Status;
type AcpiTableGet = extern "efiapi" fn(usize, *mut *mut AcpiTableHeader, *mut u32, *mut usize) -> efi::Status;
type AcpiTableRegisterNotify = extern "efiapi" fn(bool, *const AcpiNotifyFnExt) -> efi::Status;

impl AcpiTableProtocol {
    pub(crate) fn new() -> Self {
        Self { install_table: Self::install_acpi_table_ext, uninstall_table: Self::uninstall_acpi_table_ext }
    }

    /// Installs an ACPI table into the XSDT.
    ///
    /// This function generally matches the behavior of EFI_ACPI_TABLE_PROTOCOL.InstallAcpiTable() API in the UEFI spec 2.10
    /// section 20.2. Refer to the UEFI spec description for details on input parameters.
    ///
    /// This implementation only supports ACPI 2.0+.
    ///
    /// # Errors
    ///
    /// Returns [`INVALID_PARAMETER`](r_efi::efi::Status::INVALID_PARAMETER) the table buffer is null or too small to contain the ACPI table header.
    /// Returns [`UNSUPPORTED`](r_efi::efi::Status::UNSUPPORTED) if the system page size is not 64B-aligned (required for FACS in ACPI 2.0+).
    /// Returns [`UNSUPPORTED`](r_efi::efi::Status::UNSUPPORTED) if boot services cannot install the given table format.
    /// Returns [`OUT_OF_RESOURCES`](r_efi::efi::Status::OUT_OF_RESOURCES) if allocating memory for the table fails.
    /// Returns [`ALREADY_STARTED`](r_efi::efi::Status::ALREADY_STARTED) if the FADT already exists and `install` is called on a FADT again.
    /// Returns [`NOT_STARTED`](r_efi::efi::Status::NOT_STARTED) if memory or boot services are not properly initialized.
    extern "efiapi" fn install_acpi_table_ext(
        _protocol: *const AcpiTableProtocol,
        acpi_table_buffer: *const c_void,
        acpi_table_buffer_size: usize,
        table_key: *mut usize,
    ) -> efi::Status {
        if acpi_table_buffer.is_null() || acpi_table_buffer_size < 4 {
            return efi::Status::INVALID_PARAMETER;
        }

        if table_key.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }

        if acpi_table_buffer_size < mem::size_of::<AcpiTableHeader>() {
            return efi::Status::INVALID_PARAMETER;
        }

        // The size of the allocated table buffer must be large enough to store the whole table.
        // SAFETY: `acpi_table_buffer` is checked non-null and large enough to read an AcpiTableHeader.
        let table_header = unsafe { (*(acpi_table_buffer as *const AcpiTableHeader)).clone() };
        let tbl_length = table_header.length as usize;
        if tbl_length != acpi_table_buffer_size {
            return efi::Status::INVALID_PARAMETER;
        }

        // The size of the allocated table buffer must be large enough to store the table, for known table types.
        let signature = table_header.signature;
        let min_size = signature::acpi_table_min_size(signature);
        if tbl_length < min_size {
            return efi::Status::INVALID_PARAMETER;
        }

        if let Some(global_mm) = STANDARD_ACPI_PROVIDER.memory_manager.get() {
            // SAFETY: `acpi_table_buffer` has been validated as non-null and of sufficient size above.
            let acpi_table =
                unsafe { AcpiTable::new_from_ptr(acpi_table_buffer as *const AcpiTableHeader, None, global_mm) };

            if let Ok(table) = acpi_table {
                let signature = table.signature();
                let install_result = STANDARD_ACPI_PROVIDER.install_acpi_table(table);

                match install_result {
                    Ok(key) => {
                        // SAFETY: The caller must ensure the buffer passed in for the key is appropriately sized and non-null.
                        unsafe { *table_key = key.0 };
                        log::trace!(
                            "ACPI protocol: Successfully installed table with signature: 0x{:08X}, key: {}",
                            signature,
                            key.0
                        );
                    }
                    Err(e) => {
                        log::error!(
                            "ACPI protocol: Install failed with error {:?} for table with signature: 0x{:08X}",
                            e,
                            signature,
                        );
                        return e.into();
                    }
                }
                efi::Status::SUCCESS
            } else {
                efi::Status::OUT_OF_RESOURCES
            }
        } else {
            efi::Status::NOT_STARTED
        }
    }

    /// Removes an ACPI table from the XSDT.
    ///
    /// This function generally matches the behavior of EFI_ACPI_TABLE_PROTOCOL.UninstallAcpiTable() API in the UEFI spec 2.10
    /// section 20.2. Refer to the UEFI spec description for details on input parameters.
    ///
    /// This implementation only supports ACPI 2.0+.
    ///
    /// # Errors
    ///
    /// Returns [`INVALID_PARAMETER`](r_efi::efi::Status::INVALID_PARAMETER) if the table key does not correspond to an installed table.
    /// Returns [`OUT_OF_RESOURCES`](r_efi::efi::Status::OUT_OF_RESOURCES) if memory operations fail.
    extern "efiapi" fn uninstall_acpi_table_ext(_protocol: *const AcpiTableProtocol, table_key: usize) -> efi::Status {
        match STANDARD_ACPI_PROVIDER.uninstall_acpi_table(TableKey(table_key)) {
            Ok(_) => {
                log::trace!("ACPI protocol: Successfully uninstalled table with key: {}", table_key);
                efi::Status::SUCCESS
            }
            Err(e) => {
                log::error!("ACPI protocol: Failed to uninstall table with key: {} - error: {:?}", table_key, e);
                e.into()
            }
        }
    }
}

/// Custom protocol to enable non-AML ACPI SDT functionality.
#[repr(C)]
pub struct AcpiGetProtocol {
    pub version: u32,
    pub get_table: AcpiTableGet,
    pub register_notify: AcpiTableRegisterNotify,
}

// SAFETY: `AcpiGetProtocol` matches the C layout and behavior of the custom-defined EFI_ACPI_GET_PROTOCOL. (Not a UEFI spec protocol.)
unsafe impl ProtocolInterface for AcpiGetProtocol {
    const PROTOCOL_GUID: patina::BinaryGuid = patina::BinaryGuid::from_string("7F3C1A92-8B4E-4D2F-A6C9-3E12F4B8D7C1");
}

impl AcpiGetProtocol {
    pub(crate) fn new() -> Self {
        Self {
            version: ACPI_VERSIONS_GTE_2,
            get_table: Self::get_acpi_table_ext,
            register_notify: Self::register_notify_ext,
        }
    }
}

impl AcpiGetProtocol {
    /// Returns a requested ACPI table.
    ///
    /// This function generally matches the behavior of EFI_ACPI_SDT_PROTOCOL.GetAcpiTable() API in the PI spec 1.8
    /// section 9.1. Refer to the PI spec description for details on input parameters.
    ///
    /// This implementation only supports ACPI 2.0+.
    ///
    /// # Errors
    ///
    /// Returns [`INVALID_PARAMETER`](r_efi::efi::Status::INVALID_PARAMETER) the index is out of bounds of the list of installed tables.
    /// Returns [`INVALID_PARAMETER`](r_efi::efi::Status::INVALID_PARAMETER) any input or output parameters are null.
    /// Returns [`OUT_OF_RESOURCES`](r_efi::efi::Status::OUT_OF_RESOURCES) if memory operations fail.
    extern "efiapi" fn get_acpi_table_ext(
        index: usize,
        table: *mut *mut AcpiTableHeader,
        version: *mut u32,
        table_key: *mut usize,
    ) -> efi::Status {
        if table.is_null() || version.is_null() || table_key.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }

        match STANDARD_ACPI_PROVIDER.get_table_at_idx(index) {
            Ok((key_at_idx, table_at_idx)) => {
                // SAFETY: table_info is valid and output pointers have been checked for null
                // We only support ACPI versions >= 2.0
                // SAFETY: We check that `version` is non-null above.
                unsafe { *version = ACPI_VERSIONS_GTE_2 };
                // SAFETY: We check that `table_key` is non-null above.
                unsafe { *table_key = key_at_idx.0 };

                // SAFETY: We check that `table` is non-null above.
                unsafe { *table = table_at_idx.as_mut_ptr() };
                log::trace!(
                    "ACPI protocol: Successfully retrieved table at index {} with key: {} and signature: 0x{:08X}",
                    index,
                    key_at_idx.0,
                    table_at_idx.signature()
                );
                efi::Status::SUCCESS
            }
            Err(e) => {
                log::error!("ACPI protocol: Failed to get table at index {} with error: {:?}", index, e);
                e.into()
            }
        }
    }

    /// Register or unregister a callback when an ACPI table is installed.
    ///
    /// This function generally matches the behavior of EFI_ACPI_SDT_PROTOCOL.RegisterNotify() API in the PI spec 1.8
    /// section 9.1. Refer to the PI spec description for details on input parameters.
    ///
    /// This implementation only supports ACPI 2.0+.
    ///
    /// # Errors
    ///
    /// Returns [`INVALID_PARAMETER`](r_efi::efi::Status::INVALID_PARAMETER) if there is an attempt to unregister a notify function that was never registered.
    /// Returns [`INVALID_PARAMETER`](r_efi::efi::Status::INVALID_PARAMETER) if the notify function pointer is null or does not match the standard notify function signature.
    extern "efiapi" fn register_notify_ext(register: bool, notify_fn: *const AcpiNotifyFnExt) -> efi::Status {
        // SAFETY: the caller must pass in a valid pointer to a notify function
        let rust_fn: AcpiNotifyFn = match unsafe { notify_fn.as_ref() } {
            // SAFETY: The function points to an `AcpiNotifyFnExt`, which has the same signature as `AcpiNotifyFn`.
            Some(ptr) => unsafe { core::mem::transmute::<*const AcpiNotifyFnExt, AcpiNotifyFn>(ptr) },
            None => {
                return efi::Status::INVALID_PARAMETER;
            }
        };

        match STANDARD_ACPI_PROVIDER.register_notify(register, rust_fn) {
            Ok(_) => efi::Status::SUCCESS,
            Err(err) => err.into(),
        }
    }
}

type AcpiNotifyFnExt = fn(*const AcpiTableHeader, u32, usize) -> efi::Status;

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;

    #[test]
    fn acpi_table_protocol_creation() {
        let protocol = AcpiTableProtocol::new();
        assert_eq!(protocol.install_table as usize, AcpiTableProtocol::install_acpi_table_ext as *const () as usize);
        assert_eq!(
            protocol.uninstall_table as usize,
            AcpiTableProtocol::uninstall_acpi_table_ext as *const () as usize
        );
    }

    #[test]
    fn test_install_acpi_table_ext_error_cases() {
        // Test null buffer.
        let status =
            AcpiTableProtocol::install_acpi_table_ext(core::ptr::null(), core::ptr::null(), 0, core::ptr::null_mut());
        assert_eq!(status, efi::Status::INVALID_PARAMETER);

        // Test buffer too small.
        let dummy_table: [u8; 2] = [0; 2];
        let mut table_key: usize = 0;
        let status = AcpiTableProtocol::install_acpi_table_ext(
            &AcpiTableProtocol::new(),
            dummy_table.as_ptr() as *const c_void,
            dummy_table.len(),
            &mut table_key as *mut usize,
        );
        assert_eq!(status, efi::Status::INVALID_PARAMETER);

        // Test table key null.
        let dummy_table: [u8; 8] = [0; 8];
        let status = AcpiTableProtocol::install_acpi_table_ext(
            &AcpiTableProtocol::new(),
            dummy_table.as_ptr() as *const c_void,
            dummy_table.len(),
            core::ptr::null_mut(),
        );
        assert_eq!(status, efi::Status::INVALID_PARAMETER);

        // Test table length mismatch.
        let dummy_table: [u8; 8] = [100; 8]; // If this is the table buffer, the length is not 16.
        let mut table_key: usize = 0;
        let status = AcpiTableProtocol::install_acpi_table_ext(
            &AcpiTableProtocol::new(),
            dummy_table.as_ptr() as *const c_void,
            16, // Incorrect length,
            &mut table_key as *mut usize,
        );
        assert_eq!(status, efi::Status::INVALID_PARAMETER);

        // Test table smaller than known minimum size.
        let dummy_table: [u8; 8] = [b'F', b'A', b'C', b'S', 0, 0, 0, 15]; // FACS minimum size is larger than 15.
        let mut table_key: usize = 0;
        let status = AcpiTableProtocol::install_acpi_table_ext(
            &AcpiTableProtocol::new(),
            dummy_table.as_ptr() as *const c_void,
            dummy_table.len(),
            &mut table_key as *mut usize,
        );
        assert_eq!(status, efi::Status::INVALID_PARAMETER);

        // Test memory manager not initialized.
        let dummy_table: [u8; 36] = [
            // Signature "TEST"
            b'T', b'E', b'S', b'T', // 0..3
            // Length = 36 (0x24) little-endian
            36, 0, 0, 0, // 4..7
            // Revision
            1, // 8
            // Checksum (calculated so sum = 0)
            0xE5, // 9
            // OEM ID
            b'O', b'E', b'M', b'I', b'D', b' ', // 10..15
            // OEM Table ID "
            b'O', b'T', b'A', b'B', b'L', b'E', b' ', b' ', // 16..23
            // OEM Revision
            1, 0, 0, 0, // 24..27
            // Creator ID "CRID"
            b'C', b'R', b'I', b'D', // 28..31
            // Creator Revision
            1, 0, 0, 0, // 32..35
        ];
        // This should pass other table checks.
        let mut table_key: usize = 0;
        let status = AcpiTableProtocol::install_acpi_table_ext(
            &AcpiTableProtocol::new(),
            dummy_table.as_ptr() as *const c_void,
            dummy_table.len(),
            &mut table_key as *mut usize,
        );
        assert_eq!(status, efi::Status::NOT_STARTED);
    }

    #[test]
    fn test_acpi_get_init() {
        let protocol = AcpiGetProtocol::new();
        assert_eq!(protocol.version, ACPI_VERSIONS_GTE_2);
        assert_eq!(protocol.get_table as usize, AcpiGetProtocol::get_acpi_table_ext as *const () as usize);
        assert_eq!(protocol.register_notify as usize, AcpiGetProtocol::register_notify_ext as *const () as usize);
    }

    #[test]
    fn test_get_table_ext_error_cases() {
        // Test null output parameters.
        let status =
            AcpiGetProtocol::get_acpi_table_ext(0, core::ptr::null_mut(), core::ptr::null_mut(), core::ptr::null_mut());
        assert_eq!(status, efi::Status::INVALID_PARAMETER);
    }

    #[test]
    fn test_register_notify_ext_error_cases() {
        // Test null notify function.
        let status = AcpiGetProtocol::register_notify_ext(true, core::ptr::null());
        assert_eq!(status, efi::Status::INVALID_PARAMETER);
    }
}
