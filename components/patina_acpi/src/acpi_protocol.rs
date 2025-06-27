//! ACPI C Protocol Definitions.
//!
//! Wrappers for the C ACPI protocols to call into Rust ACPI implementations.

use crate::acpi_table::{AcpiTableHeader, StandardAcpiTable};
use crate::signature::ACPI_VERSIONS_GTE_2;

use alloc::collections::btree_map::BTreeMap;
use core::ffi::c_void;
use patina_sdk::uefi_protocol::ProtocolInterface;
use r_efi::efi;
use spin::rwlock::RwLock;

use crate::service::{AcpiNotifyFn, AcpiProvider};
use crate::{
    acpi::ACPI_TABLE_INFO,
    acpi_table::AcpiFacs,
    signature::{self},
};

/// Corresponds to the ACPI Table Protocol as defined in UEFI spec.
#[repr(C)]
pub(crate) struct AcpiTableProtocol {
    install_table: AcpiTableInstall,
    uninstall_table: AcpiTableUninstall,
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
    extern "efiapi" fn install_acpi_table_ext(
        _protocol: *const AcpiTableProtocol,
        acpi_table_buffer: *const c_void,
        acpi_table_buffer_size: usize,
        table_key: *mut usize,
    ) -> efi::Status {
        use core::{mem, ptr::NonNull};
        if acpi_table_buffer.is_null() || acpi_table_buffer_size < 4 {
            return efi::Status::INVALID_PARAMETER;
        }

        let signature = unsafe {
            // SAFETY: acpi_table_buffer is checked non-null and large enough to read a u32
            // The signature is always the first field on any ACPI table
            *(acpi_table_buffer as *const u32)
        };

        // Special handling for FACS, which has a different format than other ACPI tables
        if signature == signature::FACS {
            if acpi_table_buffer_size < mem::size_of::<AcpiFacs>() {
                return efi::Status::INVALID_PARAMETER;
            }

            let facs = unsafe {
                // SAFETY: size was checked above, and pointer is valid
                &mut *(acpi_table_buffer as *mut AcpiFacs)
            };

            if let Err(e) = ACPI_TABLE_INFO.install_facs(facs) {
                e.into()
            } else {
                // The FACS doesn't have an associated key
                efi::Status::SUCCESS
            }
        } else {
            if acpi_table_buffer_size < mem::size_of::<AcpiTableHeader>() {
                return efi::Status::INVALID_PARAMETER;
            }

            // SAFETY: acpi_table_buffer is checked non-null and large enough to read an AcpiTableHeader.
            let acpi_table = unsafe { CAcpiTable::from_ptr(acpi_table_buffer) };

            match ACPI_TABLE_INFO.install_acpi_table(&acpi_table) {
                Ok(key) => {
                    if let Some(key_ptr) = NonNull::new(table_key) {
                        // SAFETY: `key_ptr` is checked to be non-null
                        // The caller must ensure the buffer is writeable
                        unsafe { *key_ptr.as_ptr() = key };
                    }
                    efi::Status::SUCCESS
                }
                Err(e) => e.into(),
            }
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
        match ACPI_TABLE_INFO.uninstall_acpi_table(table_key) {
            Ok(_) => efi::Status::SUCCESS,
            Err(e) => e.into(),
        }
    }
}

/// Corresponds to the ACPI SDT Protocol as defined in PI spec.
#[repr(C)]
pub(crate) struct AcpiSdtProtocol {
    get_table: AcpiTableGet,
    register_notify: AcpiTableRegisterNotify,
    // Maps between Rust-side function IDs and C-side function pointers
    id_to_fn: RwLock<BTreeMap<*const AcpiNotifyFnExt, usize>>,
}

unsafe impl ProtocolInterface for AcpiSdtProtocol {
    const PROTOCOL_GUID: efi::Guid =
        efi::Guid::from_fields(0xeb97088e, 0xcfdf, 0x49c6, 0xbe, 0x4b, &[0xd9, 0x06, 0xa5, 0xb2, 0x0e, 0x86]);
}

impl AcpiSdtProtocol {
    pub(crate) fn new() -> Self {
        Self {
            get_table: Self::get_acpi_table_ext,
            register_notify: Self::register_notify_ext,
            id_to_fn: RwLock::new(BTreeMap::new()),
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
    /// Returns [`NOT_FOUND`](r_efi::efi::Status::NOT_FOUND) if the retrieve table is not present in ACPI memory.
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
                unsafe { *version = ACPI_VERSIONS_GTE_2 };
                unsafe { *table_key = table_info.table_key };

                if table_info.physical_address.is_none() {
                    return efi::Status::NOT_FOUND;
                }

                let sdt_ptr = table_info.physical_address.unwrap() as *mut AcpiTableHeader;
                unsafe { *table = sdt_ptr };
                efi::Status::SUCCESS
            }
            Err(e) => e.into(),
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

/// Represents a `*const c_void` pointer to an ACPI table in C.
pub(crate) struct CAcpiTable {
    table_ptr: *const c_void,
}

impl CAcpiTable {
    /// Converts a `*const c_void` pointer to a `CAcpiTable`.
    ///
    /// # Safety
    /// The caller must ensure that the pointer is valid and points to an ACPI table.
    unsafe fn from_ptr(ptr: *const c_void) -> Self {
        Self { table_ptr: ptr }
    }
}

impl StandardAcpiTable for CAcpiTable {
    /// # Safety
    /// The caller must ensure that the pointer is valid and points to an ACPI table.
    fn header(&self) -> &AcpiTableHeader {
        // SAFETY: The first field of any ACPI table is the header.
        unsafe { &*(self.table_ptr as *const AcpiTableHeader) }
    }
}
