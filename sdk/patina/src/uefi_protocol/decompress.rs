//! UEFI Decompress Protocol implementation.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use core::ffi::c_void;

use crate::uefi_protocol::ProtocolInterface;
use mu_rust_helpers::uefi_decompress::{DecompressionAlgorithm, decompress_into_with_algo};
use r_efi::efi;

/// The ffi interface for the get_info function of the `EfiDecompressProtocol`.
pub type GetInfoFn =
    extern "efiapi" fn(*mut EfiDecompressProtocol, *mut c_void, u32, *mut u32, *mut u32) -> efi::Status;
/// The ffi interface for the decompress function of the `EfiDecompressProtocol`.
pub type DecompressFn = extern "efiapi" fn(
    *mut EfiDecompressProtocol,
    *const c_void,
    u32,
    *mut c_void,
    u32,
    *mut c_void,
    u32,
) -> efi::Status;

/// C struct for the EFI Decompress protocol.
#[repr(C)]
pub struct EfiDecompressProtocol {
    /// FFI interface to get information about the necessary allocation sizes for decompression.
    get_info: GetInfoFn,
    /// FFI interface to decompress data and return it.
    decompress: DecompressFn,
}

impl EfiDecompressProtocol {
    /// Calls the get_info function of the protocol.
    pub const fn new() -> Self {
        Self { get_info: Self::get_info, decompress: Self::decompress }
    }

    /// Creates a new instance of the protocol with the given function.
    pub const fn new_with(get_info: GetInfoFn, decompress: DecompressFn) -> Self {
        Self { get_info, decompress }
    }

    extern "efiapi" fn get_info(
        _: *mut EfiDecompressProtocol,
        src: *mut c_void,
        src_size: u32,
        dst_size: *mut u32,
        scratch_size: *mut u32,
    ) -> efi::Status {
        if src.is_null() | dst_size.is_null() | scratch_size.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }

        if src_size < 8 {
            return efi::Status::INVALID_PARAMETER;
        }

        // SAFETY: The data the pointer points to is at least 8 bytes long, as checked above.
        let compressed_size = unsafe { src.cast::<u32>().read_unaligned() };

        if (src_size < compressed_size + 8) || compressed_size.checked_add(8).is_none() {
            return efi::Status::INVALID_PARAMETER;
        }

        // SAFETY: The pointers are not null, as checked above.
        //         The data the pointer points to is at least 8 bytes long, as checked above.
        unsafe { dst_size.write_volatile(src.cast::<u32>().add(1).read_unaligned()) };

        // We do not need any scratch space for the rust implementation.
        // SAFETY: The pointer is not null, as checked above.
        unsafe { scratch_size.cast::<u32>().write_volatile(0) };

        efi::Status::SUCCESS
    }

    /// FFI interface to decompress data and return it.
    extern "efiapi" fn decompress(
        _: *mut EfiDecompressProtocol,
        source_buffer: *const c_void,
        source_size: u32,
        destination_buffer: *mut c_void,
        destination_size: u32,
        _scratch_buffer: *mut c_void,
        _scratch_size: u32,
    ) -> efi::Status {
        assert!(!source_buffer.is_null());
        assert!(!destination_buffer.is_null());

        let src = unsafe { core::slice::from_raw_parts(source_buffer as *const u8, source_size as usize) };
        let dst = unsafe { core::slice::from_raw_parts_mut(destination_buffer as *mut u8, destination_size as usize) };

        match decompress_into_with_algo(src, dst, DecompressionAlgorithm::UefiDecompress) {
            Ok(()) => efi::Status::SUCCESS,
            Err(_) => efi::Status::INVALID_PARAMETER,
        }
    }
}

impl Default for EfiDecompressProtocol {
    fn default() -> Self {
        Self::new()
    }
}

unsafe impl ProtocolInterface for EfiDecompressProtocol {
    const PROTOCOL_GUID: efi::Guid =
        efi::Guid::from_fields(0xd8117cfe, 0x94A6, 0x11D4, 0x9A, 0x3A, &[0x00, 0x90, 0x27, 0x3F, 0xC1, 0x4D]);
}
