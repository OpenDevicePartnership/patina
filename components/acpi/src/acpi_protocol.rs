//! ACPI C Protocol Definitions.
//!
//! Wrappers for the C ACPI protocols to call into Rust ACPI implementations.
use crate::alloc::vec;

use core::{ffi::c_void, ptr};
use patina_sdk::uefi_protocol::ProtocolInterface;
use r_efi::efi;

use crate::service::{AcpiNotifyFn, AcpiProvider};
use crate::{
    acpi::ACPI_TABLE_INFO,
    acpi_table::{AcpiFacs, AcpiTable},
    error::AcpiError,
    signature::{self, ACPI_HEADER_LEN},
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
type AcpiTableGet = extern "efiapi" fn(usize, *mut *mut AcpiSdtHeader, *mut u32, *mut usize) -> efi::Status;
type AcpiTableRegisterNotify = extern "efiapi" fn(bool, *const AcpiNotifyFnExt) -> efi::Status;

impl AcpiTableProtocol {
    pub(crate) fn new() -> Self {
        Self { install_table: Self::install_acpi_table_ext, uninstall_table: Self::uninstall_acpi_table_ext }
    }

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

            if let Err(e) = ACPI_TABLE_INFO.install_acpi_table(facs) {
                acpi_error_to_efi_error(e)
            } else {
                // The FACS doesn't have an associated key
                efi::Status::SUCCESS
            }
        } else {
            if acpi_table_buffer_size < mem::size_of::<AcpiTable>() {
                return efi::Status::INVALID_PARAMETER;
            }

            let acpi_table = unsafe {
                // SAFETY: pointer is valid and large enough for AcpiTable
                &mut *(acpi_table_buffer as *mut AcpiTable)
            };

            // Copy non-header data into the `data` field of AcpiTable
            // SAFETY: `acpi_table_buffer` has been checked to be non-null and a valid length
            let body_len = acpi_table.length as usize - ACPI_HEADER_LEN;
            let body_src = unsafe { (acpi_table_buffer as *const u8).add(ACPI_HEADER_LEN) };
            let mut body_data = vec![0u8; body_len];
            unsafe {
                ptr::copy_nonoverlapping(body_src, body_data.as_mut_ptr(), body_len);
            }

            acpi_table.physical_address = Some(acpi_table_buffer as usize);

            match ACPI_TABLE_INFO.install_acpi_table(acpi_table) {
                Ok(key) => {
                    if let Some(key_ptr) = NonNull::new(table_key) {
                        // SAFETY: `key_ptr` is checked to be non-null
                        // The caller must ensure the buffer is writeable
                        unsafe { *key_ptr.as_ptr() = key };
                    }
                    efi::Status::SUCCESS
                }
                Err(e) => acpi_error_to_efi_error(e),
            }
        }
    }

    extern "efiapi" fn uninstall_acpi_table_ext(_protocol: *const AcpiTableProtocol, table_key: usize) -> efi::Status {
        match ACPI_TABLE_INFO.uninstall_acpi_table(table_key) {
            Ok(_) => efi::Status::SUCCESS,
            Err(e) => acpi_error_to_efi_error(e),
        }
    }
}

/// Converts a Rust AcpiError to a standard EFI error.
fn acpi_error_to_efi_error(error: AcpiError) -> efi::Status {
    match error {
        AcpiError::AllocationFailed => efi::Status::OUT_OF_RESOURCES,
        AcpiError::FacsUefiNot64BAligned => efi::Status::UNSUPPORTED,
        AcpiError::InvalidSignature => efi::Status::INVALID_PARAMETER,
        AcpiError::FadtAlreadyInstalled => efi::Status::ALREADY_STARTED,
        AcpiError::InstallTableFailed => efi::Status::UNSUPPORTED,
        AcpiError::InvalidTableKey => efi::Status::INVALID_PARAMETER,
        AcpiError::InvalidTableIndex => efi::Status::INVALID_PARAMETER,
        AcpiError::InvalidNotifyUnregister => efi::Status::INVALID_PARAMETER,
        AcpiError::FreeFailed => efi::Status::OUT_OF_RESOURCES,
        AcpiError::XsdtNotInitialized => efi::Status::UNSUPPORTED,
        AcpiError::InvalidTableFormat => efi::Status::INVALID_PARAMETER,
        AcpiError::HobTableNotInstalled => efi::Status::UNSUPPORTED,
        AcpiError::InvalidTableLength => efi::Status::INVALID_PARAMETER,
        AcpiError::InvalidXsdtEntry => efi::Status::INVALID_PARAMETER,
        AcpiError::TableNotifyFailed => efi::Status::INVALID_PARAMETER,
    }
}

/// Corresponds to the ACPI SDT Protocol as defined in PI spec.
#[repr(C)]
pub(crate) struct AcpiSdtProtocol {
    get_table: AcpiTableGet,
    register_notify: AcpiTableRegisterNotify,
}

unsafe impl ProtocolInterface for AcpiSdtProtocol {
    const PROTOCOL_GUID: efi::Guid =
        efi::Guid::from_fields(0xeb97088e, 0xcfdf, 0x49c6, 0xbe, 0x4b, &[0xd9, 0x06, 0xa5, 0xb2, 0x0e, 0x86]);
}

impl AcpiSdtProtocol {
    pub(crate) fn new() -> Self {
        Self { get_table: Self::get_acpi_table_ext, register_notify: Self::register_notify_ext }
    }
}

impl AcpiSdtProtocol {
    extern "efiapi" fn get_acpi_table_ext(
        index: usize,
        table: *mut *mut AcpiSdtHeader,
        version: *mut u32,
        table_key: *mut usize,
    ) -> efi::Status {
        if table.is_null() || version.is_null() || table_key.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }

        match ACPI_TABLE_INFO.get_acpi_table(index) {
            Ok(table_info) => {
                // SAFETY: table_info is valid and output pointers have been checked for null
                // We only support ACPI versions >= 2.0
                unsafe { *version = ((1 << 2) | (1 << 3) | (1 << 4) | (1 << 5)) as u32 };
                unsafe { *table_key = table_info.table_key };
                let sdt_ptr = table_info as *const AcpiTable as *mut AcpiSdtHeader;
                unsafe { *table = sdt_ptr };
            }
            Err(e) => return acpi_error_to_efi_error(e),
        }
        efi::Status::SUCCESS
    }

    extern "efiapi" fn register_notify_ext(register: bool, notify_fn: *const AcpiNotifyFnExt) -> efi::Status {
        // SAFETY: the caller must pass in a valid pointer to a notify function
        let rust_fn: AcpiNotifyFn = match unsafe { notify_fn.as_ref() } {
            Some(ptr) => unsafe { core::mem::transmute::<*const AcpiNotifyFnExt, AcpiNotifyFn>(ptr) },
            None => return efi::Status::INVALID_PARAMETER,
        };

        match ACPI_TABLE_INFO.register_notify(register, rust_fn) {
            Ok(_) => efi::Status::SUCCESS,
            Err(err) => acpi_error_to_efi_error(err),
        }
    }
}

/// C representation of ACPI table header.
/// It is roughly equivalent to the Rust `AcpiTable` struct.
#[repr(C, packed)]
struct AcpiSdtHeader {
    signature: u32,
    length: u32,
    revision: u8,
    checksum: u8,
    oem_id: [u8; 6],
    oem_table_id: [u8; 8],
    oem_revision: u32,
    creator_id: u32,
    creator_revision: u32,
}

type AcpiNotifyFnExt = fn(*const AcpiSdtHeader, u32, usize) -> efi::Status;
