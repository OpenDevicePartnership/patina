//! ACPI C Protocol Definitions.
//!
//! Wrappers for the C ACPI protocols to call into Rust ACPI implementations.
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

use crate::acpi_table::{AcpiFacs, AcpiFadt, AcpiTable, AcpiTableHeader};
use crate::signature::{self, ACPI_VERSIONS_GTE_2};

use alloc::collections::btree_map::BTreeMap;
use core::ffi::c_void;
use core::mem;
use patina_sdk::uefi_protocol::ProtocolInterface;
use r_efi::efi;
use spin::rwlock::RwLock;

use crate::acpi::{self, ACPI_TABLE_INFO};
use crate::service::{AcpiNotifyFn, AcpiProvider, TableKey};

/// Corresponds to the ACPI Table Protocol as defined in UEFI spec.
#[repr(C)]
pub struct AcpiTableProtocol {
    pub install_table: AcpiTableInstall,
    pub uninstall_table: AcpiTableUninstall,
}

unsafe impl ProtocolInterface for AcpiTableProtocol {
    const PROTOCOL_GUID: efi::Guid =
        efi::Guid::from_fields(0xffe06bdd, 0x6107, 0x46a6, 0x7b, 0xb2, &[0x5a, 0x9c, 0x7e, 0xc5, 0x27, 0x5c]);
}

// C function interfaces for ACPI Table Protocol and ACPI SDT Protocol.
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

        if acpi_table_buffer_size < mem::size_of::<AcpiTableHeader>() {
            return efi::Status::INVALID_PARAMETER;
        }

        // The size of the allocated table buffer must be large enough to store the whole table.
        let tbl_length = unsafe { (*(acpi_table_buffer as *const AcpiTableHeader)).length } as usize;
        if tbl_length > acpi_table_buffer_size {
            return efi::Status::INVALID_PARAMETER;
        }

        // SAFETY: acpi_table_buffer is checked non-null and large enough to read an AcpiTableHeader.
        if let Some(global_mm) = ACPI_TABLE_INFO.memory_manager.get() {
            let acpi_table = unsafe { AcpiTable::new_from_ptr(acpi_table_buffer as *const AcpiTableHeader, global_mm) };

            if let Ok(table) = acpi_table {
                let install_result = match table.signature() {
                    signature::FACS => ACPI_TABLE_INFO.install_facs(table),
                    signature::FADT => ACPI_TABLE_INFO.install_fadt(table),
                    signature::DSDT => ACPI_TABLE_INFO.install_dsdt(table),
                    _ => ACPI_TABLE_INFO.install_standard_table(table),
                };

                match install_result {
                    Ok(key) => {
                        // SAFETY: The caller must ensure the buffer passed in for the key is appropriately sized and non-null.
                        unsafe { *table_key = key.0 };
                    }
                    Err(e) => {
                        log::info!("Protocol install failed: {:?} for table with signature {}", e, table.signature());
                        return e.into();
                    }
                }

                let publish_result = ACPI_TABLE_INFO.publish_tables();
                if let Err(e) = publish_result {
                    log::info!("Failed to publish ACPI tables: {:?}", e);
                    return e.into();
                }

                let notify_result = ACPI_TABLE_INFO.notify_acpi_list(TableKey(unsafe { *table_key }));
                if let Err(e) = notify_result {
                    log::info!("Failed to notify ACPI list: {:?}", e);
                    return e.into();
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
        match ACPI_TABLE_INFO.uninstall_acpi_table(TableKey(table_key)) {
            Ok(_) => efi::Status::SUCCESS,
            Err(e) => e.into(),
        }
    }
}

/// Corresponds to the ACPI SDT Protocol as defined in PI spec.
#[repr(C)]
pub struct AcpiSdtProtocol {
    pub version: u32,
    pub get_table: AcpiTableGet,
    pub register_notify: AcpiTableRegisterNotify,
}

unsafe impl ProtocolInterface for AcpiSdtProtocol {
    const PROTOCOL_GUID: efi::Guid =
        efi::Guid::from_fields(0xeb97088e, 0xcfdf, 0x49c6, 0xbe, 0x4b, &[0xd9, 0x06, 0xa5, 0xb2, 0x0e, 0x86]);
}

impl AcpiSdtProtocol {
    pub(crate) fn new() -> Self {
        Self {
            version: ACPI_VERSIONS_GTE_2,
            get_table: Self::get_acpi_table_ext,
            register_notify: Self::register_notify_ext,
        }
    }
}

impl AcpiSdtProtocol {
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

        match ACPI_TABLE_INFO.get_table_at_idx(index) {
            Ok(table_info) => {
                // SAFETY: table_info is valid and output pointers have been checked for null
                // We only support ACPI versions >= 2.0
                let (key_at_idx, table_at_idx) = table_info;
                unsafe { *version = ACPI_VERSIONS_GTE_2 };
                unsafe { *table_key = key_at_idx.0 };

                let sdt_ptr = table_at_idx.as_mut_ptr();
                unsafe { *table = sdt_ptr };
                efi::Status::SUCCESS
            }
            Err(e) => {
                log::info!("get_acpi_table from ACPI protocol failed with error {:?}", e);
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
        log::info!("Registering ACPI notify function: {}", register);
        // SAFETY: the caller must pass in a valid pointer to a notify function
        let rust_fn: AcpiNotifyFn = match unsafe { notify_fn.as_ref() } {
            Some(ptr) => unsafe { core::mem::transmute::<*const AcpiNotifyFnExt, AcpiNotifyFn>(ptr) },
            None => return efi::Status::INVALID_PARAMETER,
        };

        match ACPI_TABLE_INFO.register_notify(register, rust_fn) {
            Ok(_) => efi::Status::SUCCESS,
            Err(err) => err.into(),
        }
    }
}

type AcpiNotifyFnExt = fn(*const AcpiTableHeader, u32, usize) -> efi::Status;
